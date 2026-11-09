//! Update check: asks GitHub for the latest published release and compares it with this
//! build's version. One HTTPS request to api.github.com; nothing is downloaded or installed.

use std::fmt;
use std::time::Duration;

use anyhow::{Context, bail};

/// GitHub repository the releases are published in.
pub const REPO: &str = "joreeves/rust-dirstat";
/// Set this environment variable (to anything but `0`) to stop the window from checking at
/// startup.
pub const OPT_OUT_ENV: &str = "DIRSTAT_NO_UPDATE_CHECK";

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

/// Asks GitHub for the latest release. Errors cover being offline, proxies, rate limits, etc.
pub fn check() -> anyhow::Result<Outcome> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(format!("{}/{}", crate::APP_NAME, env!("CARGO_PKG_VERSION")))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .http_status_as_error(false)
        .build()
        .into();
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
    let release = Release { version, tag: tag.to_string(), url };
    Ok(if version > current { Outcome::Available(release) } else { Outcome::UpToDate(release) })
}

/// True when the startup check is turned off through [`OPT_OUT_ENV`].
pub fn disabled() -> bool {
    std::env::var_os(OPT_OUT_ENV).is_some_and(|v| !v.is_empty() && v != "0")
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
}
