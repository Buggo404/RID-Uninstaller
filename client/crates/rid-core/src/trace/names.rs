//! Turns the kernel's names into the ones users know: `\Device\HarddiskVolume3\x` to `C:\x`,
//! `\REGISTRY\MACHINE\x` to `HKLM\x`.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PWSTR};

pub struct Names {
    /// (device prefix in lower case, drive like `C:`)
    drives: Vec<(String, String)>,
    /// SID of the user whose hive is HKCU for this process.
    sid: String,
}

/// SID of the user this process runs as: the name of its hive under `\REGISTRY\USER`.
fn current_sid() -> String {
    // SAFETY: the token handle is closed below; the buffer is sized by the first call and
    // outlives the SID pointer read from it; the SID string is freed with LocalFree.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return String::new();
        }
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        let mut buf = vec![0u8; needed as usize];
        let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), needed, &mut needed);
        let _ = CloseHandle(token);
        let mut text = PWSTR::null();
        if got.is_err() || ConvertSidToStringSidW((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid, &mut text).is_err() {
            return String::new();
        }
        let sid = text.to_string().unwrap_or_default();
        let _ = LocalFree(Some(HLOCAL(text.as_ptr().cast())));
        sid
    }
}

impl Names {
    pub fn new() -> Names {
        let mut drives = Vec::new();
        for letter in 'A'..='Z' {
            let drive = format!("{letter}:");
            let mut buf = [0u16; 512];
            // SAFETY: `buf` is a valid output buffer of the length passed.
            let len = unsafe { QueryDosDeviceW(&HSTRING::from(drive.as_str()), Some(&mut buf)) } as usize;
            if len > 0 {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(len);
                drives.push((String::from_utf16_lossy(&buf[..end]).to_lowercase(), drive));
            }
        }
        Names { drives, sid: current_sid() }
    }

    /// `None` for paths that are not on a drive letter (pipes, mail slots, network redirectors).
    pub fn file(&self, nt_path: &str) -> Option<String> {
        // Process image names arrive in the object manager's DOS form, `\??\C:\...`.
        if let Some(dos) = nt_path.strip_prefix("\\??\\") {
            return Some(dos.to_string());
        }
        let lower = nt_path.to_lowercase();
        self.drives.iter().find_map(|(device, drive)| {
            let rest = lower.strip_prefix(device.as_str())?;
            (rest.is_empty() || rest.starts_with('\\')).then(|| format!("{drive}{}", &nt_path[device.len()..]))
        })
    }

    /// `HKLM\...` or `HKCU\...`. `None` for other users' hives and anything outside those two.
    pub fn key(&self, kernel_path: &str) -> Option<String> {
        let sep = '\\';
        let mut parts = kernel_path.trim_start_matches(sep).splitn(3, sep);
        if !parts.next()?.eq_ignore_ascii_case("REGISTRY") {
            return None;
        }
        let root = parts.next()?;
        let rest = parts.next().unwrap_or_default();
        if root.eq_ignore_ascii_case("MACHINE") {
            return Some(format!("HKLM{sep}{rest}"));
        }
        if !root.eq_ignore_ascii_case("USER") || self.sid.is_empty() {
            return None;
        }
        let (hive, below) = rest.split_once(sep).unwrap_or((rest, ""));
        if hive.eq_ignore_ascii_case(&self.sid) {
            Some(format!("HKCU{sep}{below}"))
        } else if hive.eq_ignore_ascii_case(&format!("{}_Classes", self.sid)) {
            // The per-user class registrations are mounted as their own hive.
            Some(format!("HKCU{sep}Software{sep}Classes{sep}{below}"))
        } else {
            None
        }
    }
}

impl Default for Names {
    fn default() -> Self {
        Names::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            drives: vec![
                (r"\device\harddiskvolume3".into(), "C:".into()),
                (r"\device\harddiskvolume31".into(), "D:".into()),
            ],
            sid: "S-1-5-21-1-2-3-1001".into(),
        }
    }

    #[test]
    fn device_paths_become_drive_paths() {
        let n = names();
        assert_eq!(n.file(r"\Device\HarddiskVolume3\Program Files\7-Zip\7z.exe").as_deref(), Some(r"C:\Program Files\7-Zip\7z.exe"));
        // A device name that merely starts with another one must not be mistaken for it.
        assert_eq!(n.file(r"\Device\HarddiskVolume31\Apps\a.exe").as_deref(), Some(r"D:\Apps\a.exe"));
        assert_eq!(n.file(r"\Device\HarddiskVolume3").as_deref(), Some("C:"));
        assert_eq!(n.file(r"\Device\NamedPipe\x"), None);
        assert_eq!(n.file(r"\Device\Mup\server\share\x"), None);
        assert_eq!(n.file(r"\??\C:\Windows\system32\reg.exe").as_deref(), Some(r"C:\Windows\system32\reg.exe"));
    }

    #[test]
    fn kernel_key_paths_become_hive_paths() {
        let n = names();
        assert_eq!(n.key(r"\REGISTRY\MACHINE\SOFTWARE\7-Zip").as_deref(), Some(r"HKLM\SOFTWARE\7-Zip"));
        assert_eq!(n.key(r"\REGISTRY\USER\S-1-5-21-1-2-3-1001\Software\7-Zip").as_deref(), Some(r"HKCU\Software\7-Zip"));
        assert_eq!(
            n.key(r"\REGISTRY\USER\S-1-5-21-1-2-3-1001_Classes\.7z").as_deref(),
            Some(r"HKCU\Software\Classes\.7z")
        );
        assert_eq!(n.key(r"\REGISTRY\USER\S-1-5-18\Software\X"), None);
        assert_eq!(n.key(r"\REGISTRY\USER\.DEFAULT\Software\X"), None);
        assert_eq!(n.key(r"\REGISTRY\A\{guid}\Root"), None);
        assert_eq!(n.key(r"\Device\x"), None);
    }

    #[test]
    fn this_machine_has_a_system_drive_and_a_user() {
        let n = Names::new();
        assert!(n.drives.iter().any(|(_, drive)| drive == "C:"));
        assert!(n.sid.starts_with("S-1-5-"), "{}", n.sid);
    }
}
