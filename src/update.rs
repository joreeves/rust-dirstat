//! Updates: asks GitHub for the latest published release and compares it with this build's
//! version (one HTTPS request to api.github.com). When asked to, downloads that release's
//! installer and runs it to upgrade this copy in place.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use ureq::config::ConfigBuilder;
use ureq::typestate::AgentScope;

/// GitHub repository the releases are published in.
pub const REPO: &str = "joreeves/rust-dirstat";
/// Set this environment variable (to anything but `0`) to stop the window from checking at
/// startup.
pub const OPT_OUT_ENV: &str = "DIRSTAT_NO_UPDATE_CHECK";
/// How the release workflow names the installer: `rust-dirstat-vX.Y.Z-windows-x64-setup.exe`.
const INSTALLER_SUFFIX: &str = "-windows-x64-setup.exe";

pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases")
}

/// A `major.minor.patch` version (pre-release and build suffixes are ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// Parses `1.2.3`, `v1.2.3` or `1.2.3-beta.1`.
    pub fn parse(s: &str) -> Option<Self> {
        let core = s.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let version = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// The version of this build.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml version is major.minor.patch")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Tag name, e.g. `v0.2.0`.
    pub tag: String,
    /// The release's web page (notes and downloads).
    pub url: String,
    /// The Windows installer, if the release has one.
    pub installer: Option<Asset>,
}

/// A file attached to a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Size in bytes.
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A newer release is available.
    Available(Release),
    /// This build is the latest release (or newer).
    UpToDate(Release),
    /// Nothing has been published yet.
    NoReleases,
}

/// An HTTPS client that checks certificates against the system's store; `timeouts` sets the
/// limits for the job at hand.
fn agent(timeouts: impl FnOnce(ConfigBuilder<AgentScope>) -> ConfigBuilder<AgentScope>) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .user_agent(format!("{}/{}", crate::APP_NAME, env!("CARGO_PKG_VERSION")))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .http_status_as_error(false);
    timeouts(config).build().into()
}

/// Asks GitHub for the latest release. Errors cover being offline, proxies, rate limits, etc.
pub fn check() -> anyhow::Result<Outcome> {
    let agent = agent(|c| c.timeout_global(Some(Duration::from_secs(15))));
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("could not reach GitHub")?;
    let status = response.status().as_u16();
    if status == 404 {
        return Ok(Outcome::NoReleases);
    }
    let body = response.body_mut().read_to_string().context("could not read GitHub's reply")?;
    if status != 200 {
        bail!("GitHub answered with HTTP {status}");
    }
    parse_latest(&body, Version::current())
}

/// Interprets the body of `GET /repos/{repo}/releases/latest`.
pub fn parse_latest(body: &str, current: Version) -> anyhow::Result<Outcome> {
    let json: serde_json::Value = serde_json::from_str(body).context("unexpected reply from GitHub")?;
    let tag = json["tag_name"].as_str().context("reply has no tag_name")?;
    let url = json["html_url"].as_str().map_or_else(releases_page, str::to_string);
    let version = Version::parse(tag).with_context(|| format!("release tag {tag:?} is not a version"))?;
    let installer = json["assets"].as_array().into_iter().flatten().find_map(|a| {
        let name = a["name"].as_str().filter(|n| n.ends_with(INSTALLER_SUFFIX))?;
        Some(Asset { name: name.to_string(), url: a["browser_download_url"].as_str()?.to_string(), size: a["size"].as_u64()? })
    });
    let release = Release { version, tag: tag.to_string(), url, installer };
    Ok(if version > current { Outcome::Available(release) } else { Outcome::UpToDate(release) })
}

/// True when the startup check is turned off through [`OPT_OUT_ENV`].
pub fn disabled() -> bool {
    std::env::var_os(OPT_OUT_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// True when this executable was put there by the installer, which can then upgrade it in place.
/// (Portable copies from the zip are updated by hand.)
pub fn installed_copy() -> bool {
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(is_install_dir)).unwrap_or(false)
}

/// The installer keeps its uninstaller next to the executable.
fn is_install_dir(dir: &Path) -> bool {
    dir.join("unins000.exe").is_file()
}

/// Downloads `asset` to a folder in the temp directory, storing the bytes received so far in
/// `received`. Returns the file's path once all of it has arrived.
pub fn download(asset: &Asset, received: &AtomicU64) -> anyhow::Result<PathBuf> {
    // The name comes from GitHub: only ever use it as a plain file name.
    if Path::new(&asset.name).file_name() != Some(OsStr::new(&asset.name)) {
        bail!("unexpected file name {:?}", asset.name);
    }
    let dir = std::env::temp_dir().join(format!("{}-update", crate::APP_NAME));
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let path = dir.join(&asset.name);
    let partial = dir.join(format!("{}.part", asset.name));
    let result = download_to(asset, &partial, received);
    if let Err(e) = result {
        let _ = fs::remove_file(&partial);
        return Err(e);
    }
    fs::rename(&partial, &path).with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

fn download_to(asset: &Asset, path: &Path, received: &AtomicU64) -> anyhow::Result<()> {
    let agent = agent(|c| {
        c.timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_secs(15 * 60)))
    });
    let mut response = agent.get(&asset.url).call().context("could not reach GitHub")?;
    let status = response.status().as_u16();
    if status != 200 {
        bail!("GitHub answered with HTTP {status}");
    }
    // Reading more than the release says the file has is an error. (ureq's limit errors as soon
    // as it's used up, even at the end of the body: allow one byte more to read the end.)
    let mut body = response.body_mut().with_config().limit(asset.size + 1).reader();
    let mut file = fs::File::create(path).with_context(|| format!("could not create {}", path.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = body.read(&mut buf).context("the download was interrupted")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).with_context(|| format!("could not write {}", path.display()))?;
        total += n as u64;
        received.store(total, Ordering::Relaxed);
    }
    file.sync_all().with_context(|| format!("could not write {}", path.display()))?;
    if total != asset.size {
        bail!("the download is incomplete ({total} of {} bytes)", asset.size);
    }
    Ok(())
}

/// Starts the downloaded installer to upgrade this copy in place. It shows only a progress
/// window and keeps the options chosen at the first install. With `relaunch`, it opens
/// rust-dirstat again when it's done. Exit right after this, so the installer can replace the
/// executable.
pub fn run_installer(path: &Path, relaunch: bool) -> anyhow::Result<()> {
    let mut command = std::process::Command::new(path);
    command.args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
    if relaunch {
        command.arg("/relaunch=1");
    }
    command.spawn().with_context(|| format!("could not start {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(Version::parse("1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("v0.10.0"), Some(Version(0, 10, 0)));
        assert_eq!(Version::parse("V2.0.1-beta.1"), Some(Version(2, 0, 1)));
        assert_eq!(Version::parse("1.2.3+build.5"), Some(Version(1, 2, 3)));
        for bad in ["", "1.2", "1.2.3.4", "1.x.3", "latest", "v"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn compares_latest_release_with_current() {
        let body = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/joreeves/rust-dirstat/releases/tag/v0.2.0","assets":[]}"#;
        let release = Release {
            version: Version(0, 2, 0),
            tag: "v0.2.0".into(),
            url: "https://github.com/joreeves/rust-dirstat/releases/tag/v0.2.0".into(),
            installer: None,
        };
        assert_eq!(parse_latest(body, Version(0, 1, 0)).unwrap(), Outcome::Available(release.clone()));
        assert_eq!(parse_latest(body, Version(0, 2, 0)).unwrap(), Outcome::UpToDate(release.clone()));
        assert_eq!(parse_latest(body, Version(0, 3, 0)).unwrap(), Outcome::UpToDate(release));
        // Missing html_url falls back to the releases page; bad replies are errors.
        let bare = parse_latest(r#"{"tag_name":"v9.0.0"}"#, Version(0, 1, 0)).unwrap();
        assert!(matches!(bare, Outcome::Available(r) if r.url == releases_page()));
        assert!(parse_latest("not json", Version(0, 1, 0)).is_err());
        assert!(parse_latest(r#"{"tag_name":"nightly"}"#, Version(0, 1, 0)).is_err());
        assert!(parse_latest(r#"{"message":"Not Found"}"#, Version(0, 1, 0)).is_err());
    }

    #[test]
    fn finds_the_installer_among_the_assets() {
        let body = r#"{"tag_name":"v0.4.0","assets":[
            {"name":"SHA256SUMS.txt","browser_download_url":"https://example.invalid/sums","size":212},
            {"name":"rust-dirstat-v0.4.0-windows-x64.zip","browser_download_url":"https://example.invalid/zip","size":7765504},
            {"name":"rust-dirstat-v0.4.0-windows-x64-setup.exe","browser_download_url":"https://example.invalid/setup","size":7286437}
        ]}"#;
        let Outcome::Available(release) = parse_latest(body, Version(0, 3, 0)).unwrap() else { panic!("newer") };
        assert_eq!(
            release.installer,
            Some(Asset {
                name: "rust-dirstat-v0.4.0-windows-x64-setup.exe".into(),
                url: "https://example.invalid/setup".into(),
                size: 7286437,
            })
        );
        let Outcome::Available(bare) = parse_latest(r#"{"tag_name":"v0.4.0","assets":[]}"#, Version(0, 3, 0)).unwrap() else {
            panic!("newer")
        };
        assert_eq!(bare.installer, None);
    }

    #[test]
    fn an_install_folder_has_the_uninstaller() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_install_dir(dir.path()));
        fs::write(dir.path().join("unins000.exe"), b"MZ").unwrap();
        assert!(is_install_dir(dir.path()));
        // Running from target\ in a checkout is not an installed copy.
        assert!(!installed_copy());
    }

    /// Downloads the latest release's installer from GitHub (needs the network):
    /// `cargo test --lib downloads_the_latest_installer -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_the_latest_installer() {
        let (Outcome::Available(release) | Outcome::UpToDate(release)) = check().unwrap() else { panic!("no releases") };
        let asset = release.installer.expect("the latest release has an installer");
        let received = AtomicU64::new(0);
        let path = download(&asset, &received).unwrap();
        assert_eq!(received.load(Ordering::Relaxed), asset.size);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, asset.size);
        assert_eq!(&bytes[..2], b"MZ", "a Windows executable");
        fs::remove_file(path).unwrap();
    }
}
