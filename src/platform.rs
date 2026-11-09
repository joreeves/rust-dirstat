//! Thin OS helpers. Windows gets real implementations; other platforms get conservative stubs
//! so the crate still builds there.

use std::io;
use std::path::{Path, PathBuf};

/// A mounted drive, as shown in the GUI's drive picker.
#[derive(Clone, Debug)]
pub struct DriveInfo {
    pub root: PathBuf,
    pub filesystem: String,
    pub total: u64,
    pub free: u64,
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetDiskFreeSpaceExW, GetDriveTypeW, GetFileInformationByHandle,
        GetLogicalDrives, GetVolumeInformationW,
    };
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::DriveInfo;

    fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn filesystem_name(root: &Path) -> Option<String> {
        let mut root_w = wide(root.as_os_str());
        if root_w.len() >= 2 && root_w[root_w.len() - 2] != u16::from(b'\\') {
            root_w.insert(root_w.len() - 1, u16::from(b'\\'));
        }
        let mut fs = [0u16; 64];
        // SAFETY: all pointers are valid for the duration of the call; buffer sizes are correct.
        let ok = unsafe {
            GetVolumeInformationW(
                root_w.as_ptr(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let len = fs.iter().position(|&c| c == 0).unwrap_or(fs.len());
        Some(String::from_utf16_lossy(&fs[..len]))
    }

    pub fn file_id(path: &Path) -> Option<(u32, u64)> {
        const FILE_READ_ATTRIBUTES: u32 = 0x80;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(0x7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .ok()?;
        // SAFETY: zeroed is a valid bit pattern for this plain-data struct.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is open for the duration of the call; `info` is a valid out pointer.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) };
        (ok != 0).then(|| {
            (info.dwVolumeSerialNumber, (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow))
        })
    }

    pub fn is_elevated() -> bool {
        let mut token: HANDLE = null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` is a valid out pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        // SAFETY: the token handle is valid; the buffer matches the declared size.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
        };
        // SAFETY: `token` was opened above and is closed exactly once.
        unsafe { CloseHandle(token) };
        ok != 0 && elevation.TokenIsElevated != 0
    }

    pub fn relaunch_elevated(args: &[String]) -> bool {
        let Ok(exe) = std::env::current_exe() else { return false };
        let params = args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ");
        shell_execute("runas", exe.as_os_str(), Some(&params))
    }

    pub fn open_path(path: &Path) -> bool {
        shell_execute("open", path.as_os_str(), None)
    }

    fn shell_execute(verb: &str, file: &OsStr, params: Option<&str>) -> bool {
        let verb_w = wide(verb);
        let file_w = wide(file);
        let params_w = params.map(wide);
        // SAFETY: all strings are NUL-terminated and outlive the call.
        let result = unsafe {
            ShellExecuteW(
                null_mut(),
                verb_w.as_ptr(),
                file_w.as_ptr(),
                params_w.as_ref().map_or(null(), |p| p.as_ptr()),
                null(),
                SW_SHOWNORMAL,
            )
        };
        result as isize > 32
    }

    fn quote_arg(a: &str) -> String {
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            return a.to_owned();
        }
        // Backslashes before a closing quote must be doubled (CommandLineToArgvW rules).
        let trimmed = a.trim_end_matches('\\');
        let trailing = a.len() - trimmed.len();
        format!("\"{}{}\"", trimmed.replace('"', "\\\""), "\\".repeat(trailing * 2))
    }

    pub fn drives() -> Vec<DriveInfo> {
        const DRIVE_REMOTE: u32 = 4;
        const DRIVE_CDROM: u32 = 5;
        // SAFETY: no arguments.
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = PathBuf::from(format!("{}:\\", char::from(b'A' + i as u8)));
            let root_w = wide(root.as_os_str());
            // SAFETY: NUL-terminated string.
            let kind = unsafe { GetDriveTypeW(root_w.as_ptr()) };
            if kind == DRIVE_REMOTE || kind == DRIVE_CDROM {
                continue;
            }
            let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
            // SAFETY: valid out pointers.
            if unsafe { GetDiskFreeSpaceExW(root_w.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
                continue;
            }
            let filesystem = filesystem_name(&root).unwrap_or_default();
            out.push(DriveInfo { root, filesystem, total, free });
        }
        out
    }

    /// Hides the console window when the process was started from Explorer (it owns a console
    /// nobody is looking at). Leaves it alone when launched from a terminal.
    pub fn detach_console_if_owned() {
        let mut pids = [0u32; 4];
        // SAFETY: buffer and length match.
        let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
        if count == 1 {
            // SAFETY: no arguments.
            unsafe { FreeConsole() };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::{Path, PathBuf};

    use super::DriveInfo;

    pub fn filesystem_name(_root: &Path) -> Option<String> {
        None
    }
    pub fn file_id(_path: &Path) -> Option<(u32, u64)> {
        None
    }
    pub fn is_elevated() -> bool {
        false
    }
    pub fn relaunch_elevated(_args: &[String]) -> bool {
        false
    }
    pub fn open_path(path: &Path) -> bool {
        std::process::Command::new("xdg-open").arg(path).spawn().is_ok()
    }
    pub fn drives() -> Vec<DriveInfo> {
        vec![DriveInfo { root: PathBuf::from("/"), filesystem: String::new(), total: 0, free: 0 }]
    }
    pub fn detach_console_if_owned() {}
}

pub use imp::{detach_console_if_owned, drives, file_id, filesystem_name, is_elevated, open_path, relaunch_elevated};

/// Opens Explorer with `path` selected.
pub fn reveal_in_explorer(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let dir = path.parent().unwrap_or(path);
        std::process::Command::new("xdg-open").arg(dir).spawn()?;
        Ok(())
    }
}
