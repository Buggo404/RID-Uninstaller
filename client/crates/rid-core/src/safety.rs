//! Hard limits on what Rid may delete. Every deletion goes through these checks at the
//! moment it is carried out, whatever the scanner or the UI proposed.

use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_LocalAppDataLow, FOLDERID_Profile,
    FOLDERID_ProgramData, FOLDERID_ProgramFilesX64, FOLDERID_ProgramFilesX86, FOLDERID_Programs, FOLDERID_Public,
    FOLDERID_PublicDesktop, FOLDERID_RoamingAppData, FOLDERID_Windows, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::GUID;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Denied {
    /// Not an absolute local path / not a key under HKLM or HKCU, or contains `..`.
    Malformed,
    /// A system location, or a folder that system or shared locations live in.
    Protected,
    /// Another installed program lives at or below this folder.
    OtherProgram,
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Denied::Malformed => "not a valid local path or registry key",
            Denied::Protected => "protected system or shared location",
            Denied::OtherProgram => "another installed program lives here",
        })
    }
}

impl std::error::Error for Denied {}

/// Well-known folders of this machine and user.
#[derive(Debug, Clone)]
pub struct Roots {
    pub windows: PathBuf,
    pub program_files: Vec<PathBuf>,
    pub program_data: PathBuf,
    pub profile: PathBuf,
    pub roaming: PathBuf,
    pub local: PathBuf,
    pub local_low: PathBuf,
    pub public: PathBuf,
    pub desktops: Vec<PathBuf>,
    /// Start Menu\Programs, per-user and all-users.
    pub start_menus: Vec<PathBuf>,
}

impl Roots {
    pub fn from_system() -> Roots {
        let known = |id: &GUID| -> PathBuf {
            // SAFETY: the returned buffer is valid until freed; it is copied out and freed here.
            unsafe {
                SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)
                    .ok()
                    .map(|p| {
                        let s = p.to_string().unwrap_or_default();
                        windows::Win32::System::Com::CoTaskMemFree(Some(p.as_ptr().cast()));
                        PathBuf::from(s)
                    })
                    .unwrap_or_default()
            }
        };
        Roots {
            windows: known(&FOLDERID_Windows),
            program_files: vec![known(&FOLDERID_ProgramFilesX64), known(&FOLDERID_ProgramFilesX86)],
            program_data: known(&FOLDERID_ProgramData),
            profile: known(&FOLDERID_Profile),
            roaming: known(&FOLDERID_RoamingAppData),
            local: known(&FOLDERID_LocalAppData),
            local_low: known(&FOLDERID_LocalAppDataLow),
            public: known(&FOLDERID_Public),
            desktops: vec![known(&FOLDERID_Desktop), known(&FOLDERID_PublicDesktop)],
            start_menus: vec![known(&FOLDERID_Programs), known(&FOLDERID_CommonPrograms)],
        }
    }

    /// Folders that must survive, together with everything above them.
    fn protected(&self) -> Vec<String> {
        let mut dirs = vec![
            self.windows.clone(),
            self.program_data.clone(),
            self.program_data.join("Microsoft"),
            self.program_data.join("Package Cache"),
            self.profile.clone(),
            self.roaming.clone(),
            self.roaming.join("Microsoft"),
            self.local.clone(),
            self.local.join("Microsoft"),
            self.local.join("Programs"),
            self.local.join("Packages"),
            self.local.join("Temp"),
            self.local_low.clone(),
            self.local_low.join("Microsoft"),
            self.public.clone(),
            // Rid's own records. Removing them from under itself would lose the undo history.
            self.program_data.join("Rid Uninstaller"),
            self.roaming.join("Rid Uninstaller"),
        ];
        for pf in &self.program_files {
            dirs.extend([pf.clone(), pf.join("Common Files"), pf.join("WindowsApps"), pf.join("Microsoft")]);
        }
        dirs.extend(self.desktops.iter().cloned());
        for menu in &self.start_menus {
            dirs.extend([menu.clone(), menu.join("Startup")]);
        }
        dirs.iter().filter_map(|d| norm(d)).collect()
    }
}

/// Lower-cased `c:\a\b` form for comparisons. `None` for anything that is not a plain
/// absolute path on a drive letter (relative, UNC, device paths, `..` components).
pub fn norm(path: &Path) -> Option<String> {
    let s = path.to_str()?.replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let b = s.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' || (b.len() > 2 && b[2] != b'\\') {
        return None;
    }
    let mut out = s[..2].to_ascii_lowercase();
    for part in s[2..].split('\\') {
        if !part.is_empty() && part.chars().all(|c| c == '.') {
            return None; // "." and ".." would make the text differ from the folder it names
        }
        // Windows ignores trailing dots and spaces in a name, so "Program Files." is "Program Files".
        let part = part.trim_end_matches(['.', ' ']);
        if !part.is_empty() {
            out.push('\\');
            out.push_str(&part.to_lowercase());
        }
    }
    Some(out)
}

/// `inner` is `outer` or lies below it. Both in `norm` form.
pub fn within(inner: &str, outer: &str) -> bool {
    inner == outer || inner.strip_prefix(outer).is_some_and(|rest| rest.starts_with('\\'))
}

/// Names that are system folders at the top of any drive, not only the system drive:
/// installers on D: use the same layout.
const DRIVE_ROOT_NAMES: [&str; 10] = [
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "users",
    "documents and settings",
    "$recycle.bin",
    "system volume information",
    "recovery",
    // Rid's per-volume quarantine.
    "ridquarantine",
];

/// Folders directly inside any user profile that hold the user's own files and settings.
const PROFILE_NAMES: [&str; 14] = [
    "appdata",
    r"appdata\roaming",
    r"appdata\local",
    r"appdata\locallow",
    "desktop",
    "documents",
    "downloads",
    "pictures",
    "music",
    "videos",
    "favorites",
    "onedrive",
    "saved games",
    "contacts",
];

/// May the file or folder at `path` be removed? `other_programs` are the install
/// locations of every installed program except the one being removed.
///
/// Pass a path whose parent has been resolved (see [`resolve`]), so that short names
/// and junctions cannot be used to walk around the checks.
pub fn check_path(path: &Path, roots: &Roots, other_programs: &[PathBuf]) -> Result<(), Denied> {
    let p = norm(path).ok_or(Denied::Malformed)?;
    let parts: Vec<&str> = p.split('\\').collect();
    if parts.len() < 2 {
        return Err(Denied::Protected); // a drive root
    }
    if parts.len() == 2 && DRIVE_ROOT_NAMES.contains(&parts[1]) {
        return Err(Denied::Protected);
    }
    if parts[1] == "users" {
        let below_profile = parts.get(3..).map(|rest| rest.join("\\")).unwrap_or_default();
        if parts.len() <= 3 || PROFILE_NAMES.contains(&below_profile.as_str()) {
            return Err(Denied::Protected);
        }
    }
    if norm(&roots.windows).is_some_and(|w| within(&p, &w)) {
        return Err(Denied::Protected);
    }
    if roots.protected().iter().any(|keep| within(keep, &p)) {
        return Err(Denied::Protected);
    }
    // Shell data under ...\Microsoft\Windows (Recent, INetCache, Themes) is never a leftover;
    // only the shortcuts and folders inside Start Menu\Programs are.
    for base in [&roots.roaming, &roots.local, &roots.program_data] {
        let Some(shell) = norm(&base.join(r"Microsoft\Windows")) else { continue };
        let in_start_menu = roots.start_menus.iter().filter_map(|m| norm(m)).any(|m| within(&p, &m));
        if within(&p, &shell) && !in_start_menu {
            return Err(Denied::Protected);
        }
    }
    if other_programs.iter().filter_map(|d| norm(d)).any(|other| within(&other, &p)) {
        return Err(Denied::OtherProgram);
    }
    Ok(())
}

/// Resolves the parent folder (junctions, 8.3 names) and re-attaches the last component,
/// so the result names where the entry really lives while a link itself stays a link.
pub fn resolve(path: &Path) -> PathBuf {
    match (path.parent().and_then(|p| std::fs::canonicalize(p).ok()), path.file_name()) {
        (Some(parent), Some(name)) => {
            let parent = parent.to_string_lossy();
            PathBuf::from(parent.strip_prefix(r"\\?\").unwrap_or(&parent)).join(name)
        }
        _ => path.to_path_buf(),
    }
}

/// Registry keys are written `HKLM\...` or `HKCU\...`; 32-bit view keys spell out `WOW6432Node`.
/// Returns the hive and the upper-cased path below it with any `WOW6432Node` hop removed,
/// so one set of rules covers both views.
fn key_parts(key: &str) -> Option<(&'static str, Vec<String>)> {
    let (hive, rest) = key.split_once('\\')?;
    let hive = match hive.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => "HKLM",
        "HKCU" | "HKEY_CURRENT_USER" => "HKCU",
        _ => return None,
    };
    let parts: Vec<String> =
        rest.split('\\').map(|p| p.to_uppercase()).filter(|p| p != "WOW6432NODE").collect();
    (!parts.is_empty() && parts.iter().all(|p| !p.is_empty())).then_some((hive, parts))
}

const CURRENT_VERSION: [&str; 4] = ["SOFTWARE", "MICROSOFT", "WINDOWS", "CURRENTVERSION"];

/// May the registry key (with its whole subtree) be removed?
pub fn check_key(key: &str) -> Result<(), Denied> {
    let (hive, parts) = key_parts(key).ok_or(Denied::Malformed)?;
    let p: Vec<&str> = parts.iter().map(String::as_str).collect();
    if p[0] != "SOFTWARE" || p.len() < 2 {
        // SYSTEM (services are removed through the service manager), Environment, Control Panel...
        return Err(Denied::Protected);
    }
    let ok = match p[1] {
        "MICROSOFT" => {
            // Windows and Microsoft products keep their state here. Only single entries of
            // the per-program lists may go, never a list itself or anything else.
            let cv = p.starts_with(&CURRENT_VERSION);
            (cv && p.len() == 6 && matches!(p[4], "UNINSTALL" | "APP PATHS"))
                || (cv && p.len() == 9 && p[4..6] == ["INSTALLER", "USERDATA"] && p[7] == "PRODUCTS")
                || (hive == "HKCU" && p.len() == 5 && p[2] == "INSTALLER" && matches!(p[3], "PRODUCTS" | "FEATURES"))
        }
        "CLASSES" => p.len() == 5 && p[2] == "INSTALLER" && matches!(p[3], "PRODUCTS" | "FEATURES"),
        "POLICIES" | "REGISTEREDAPPLICATIONS" | "CLIENTS" | "ODBC" => false,
        _ => true,
    };
    if ok { Ok(()) } else { Err(Denied::Protected) }
}

/// Class registrations that Windows itself owns. A program adds entries below some of these;
/// none of them is ever a program's own key.
const SHARED_CLASSES: [&str; 22] = [
    "CLSID", "INTERFACE", "TYPELIB", "APPID", "*", "DIRECTORY", "FOLDER", "DRIVE", "ALLFILESYSTEMOBJECTS", ".EXE",
    ".DLL", ".LNK", "EXEFILE", "DLLFILE", "LNKFILE", "APPLICATIONS", "SYSTEMFILEASSOCIATIONS", "LOCAL SETTINGS",
    "INSTALLER", "MIME", "PROTOCOLS", "ACTIVATABLECLASSES",
];

/// May a key that the install tracer saw being *created* be removed? Wider than
/// [`check_key`]: a traced key can sit among Windows' own (a COM class, a shell extension,
/// a file type), where name matching would never be trusted. The keys such entries hang
/// from are still off limits.
pub fn check_key_traced(key: &str) -> Result<(), Denied> {
    if check_key(key).is_ok() {
        return Ok(());
    }
    let (_, parts) = key_parts(key).ok_or(Denied::Malformed)?;
    let p: Vec<&str> = parts.iter().map(String::as_str).collect();
    let ok = p[0] == "SOFTWARE"
        && match p.get(1).copied() {
            Some("CLASSES") if p.len() < 3 => false,
            // An entry in one of the big registries, e.g. Classes\CLSID\{guid}.
            Some("CLASSES") if matches!(p[2], "CLSID" | "INTERFACE" | "TYPELIB" | "APPID" | "APPLICATIONS") => p.len() >= 4,
            // Below a class every program shares (*, Directory, Folder...), only its own verb
            // (...\shell\<verb>) or handler (...\shellex\<kind>\<name>), never the lists themselves.
            Some("CLASSES") if SHARED_CLASSES.contains(&p[2]) => {
                match (p.iter().position(|s| *s == "SHELLEX"), p.iter().position(|s| *s == "SHELL")) {
                    (Some(i), _) => p.len() >= i + 3,
                    (None, Some(i)) => p.len() >= i + 2,
                    (None, None) => false,
                }
            }
            // A file type or ProgID of its own, e.g. Classes\.7z or Classes\7-Zip.7z.
            Some("CLASSES") => true,
            // Deep inside Windows' own settings only: ...\CurrentVersion\<list>\<entry> and below.
            Some("MICROSOFT") => p.len() >= 6 && !(p.len() == 6 && p[5] == "APPROVED"),
            _ => false,
        };
    if ok { Ok(()) } else { Err(Denied::Protected) }
}

/// May a single value be removed from `key`? Only autostart entries qualify.
pub fn check_value(key: &str) -> Result<(), Denied> {
    let (_, parts) = key_parts(key).ok_or(Denied::Malformed)?;
    let p: Vec<&str> = parts.iter().map(String::as_str).collect();
    if p.len() == 5 && p.starts_with(&CURRENT_VERSION) && matches!(p[4], "RUN" | "RUNONCE") {
        Ok(())
    } else {
        Err(Denied::Protected)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn roots() -> Roots {
        let p = PathBuf::from;
        Roots {
            windows: p(r"C:\Windows"),
            program_files: vec![p(r"C:\Program Files"), p(r"C:\Program Files (x86)")],
            program_data: p(r"C:\ProgramData"),
            profile: p(r"C:\Users\me"),
            roaming: p(r"C:\Users\me\AppData\Roaming"),
            local: p(r"C:\Users\me\AppData\Local"),
            local_low: p(r"C:\Users\me\AppData\LocalLow"),
            public: p(r"C:\Users\Public"),
            desktops: vec![p(r"D:\Redirected\Desktop"), p(r"C:\Users\Public\Desktop")],
            start_menus: vec![
                p(r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs"),
                p(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs"),
            ],
        }
    }

    fn path(p: &str, others: &[&str]) -> Result<(), Denied> {
        let others: Vec<PathBuf> = others.iter().map(PathBuf::from).collect();
        check_path(Path::new(p), &roots(), &others)
    }

    #[test]
    fn program_folders_may_go() {
        for ok in [
            r"C:\Program Files\7-Zip",
            r"D:\Program Files (x86)\Everything",
            r"D:\CC Switch",
            r"C:\Users\me\AppData\Roaming\Apifox",
            r"C:\Users\me\AppData\Local\Programs\cherry-studio",
            r"C:\ProgramData\Apifox",
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\7-Zip",
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\7-Zip\7-Zip Help.lnk",
            r"C:\Users\Public\Desktop\Apifox.lnk",
        ] {
            assert_eq!(path(ok, &[]), Ok(()), "{ok}");
        }
    }

    /// Reported against Geek Uninstaller: removing Hikvision iVMS-4200 offered the whole
    /// `D:\Program Files (x86)` because the uninstaller sat directly in it.
    #[test]
    fn program_files_on_any_drive_is_protected() {
        for bad in [r"D:\Program Files (x86)", r"D:\Program Files", r"E:\PROGRAM FILES\", r"D:\Program Files (x86)."] {
            assert_eq!(path(bad, &[]), Err(Denied::Protected), "{bad}");
        }
    }

    /// Reported against Geek Uninstaller: `AppData\Roaming\Microsoft` was wiped.
    #[test]
    fn system_and_shared_locations_are_protected() {
        for bad in [
            r"C:\",
            r"D:",
            r"C:\Windows",
            r"C:\Windows\System32\drivers\x.sys",
            r"C:\Windows\Installer\{GUID}",
            r"C:\Program Files",
            r"C:\Program Files\Common Files",
            r"C:\Program Files\WindowsApps",
            r"C:\ProgramData",
            r"C:\ProgramData\Microsoft",
            r"C:\ProgramData\Microsoft\Windows",
            r"C:\ProgramData\Microsoft\Windows\Start Menu",
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs",
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup",
            r"C:\Users",
            r"C:\Users\me",
            r"C:\Users\someone-else",
            r"C:\Users\someone-else\Documents",
            r"D:\Users\x\AppData\Roaming",
            r"C:\Users\me\AppData",
            r"C:\Users\me\AppData\Roaming",
            r"C:\Users\me\AppData\Roaming\Microsoft",
            r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Recent",
            r"C:\Users\me\AppData\Local\Microsoft\Windows\INetCache",
            r"C:\Users\me\AppData\Local\Temp",
            r"C:\Users\me\AppData\Local\Programs",
            r"C:\Users\me\Downloads",
            r"D:\Redirected\Desktop",
            r"D:\Redirected",
            r"C:\Users\Public\Desktop",
            // Found by uninstalling Rid with Rid: its own records and quarantine are not leftovers.
            r"C:\ProgramData\Rid Uninstaller",
            r"C:\Users\me\AppData\Roaming\Rid Uninstaller",
            r"C:\RidQuarantine",
            r"D:\RidQuarantine",
        ] {
            assert_eq!(path(bad, &[]), Err(Denied::Protected), "{bad}");
        }
    }

    #[test]
    fn malformed_paths_are_refused() {
        for bad in ["", "relative\\dir", r"\\server\share\app", r"C:\Program Files\App\..", r"C:\a\..\Windows", r"\\.\C:\x", "C:foo"] {
            assert_eq!(path(bad, &[]), Err(Denied::Malformed), "{bad}");
        }
    }

    /// Reported against Uninstall Tool: removing a Teams component took Office with it.
    /// Reported against Revo: other games listed as leftovers of one game.
    #[test]
    fn a_folder_holding_another_program_is_refused() {
        let office = r"C:\Program Files\Microsoft Office";
        assert_eq!(path(office, &[r"C:\Program Files\Microsoft Office\root\Office16"]), Err(Denied::OtherProgram));
        assert_eq!(path(office, &[office]), Err(Denied::OtherProgram));
        assert_eq!(path(r"D:\Games", &[r"D:\Games\StarCraft", r"D:\Games\Raft"]), Err(Denied::OtherProgram));
        // A sibling is unaffected, and a prefix of a name is not a parent folder.
        assert_eq!(path(r"D:\Games\Raft", &[r"D:\Games\StarCraft", r"D:\Games\Raft 2"]), Ok(()));
    }

    #[test]
    fn registry_program_keys_may_go() {
        for ok in [
            r"HKLM\SOFTWARE\7-Zip",
            r"HKCU\Software\Apifox",
            r"HKLM\SOFTWARE\WOW6432Node\Igor Pavlov\7-Zip",
            r"HKEY_CURRENT_USER\Software\voidtools\Everything",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{23170F69-40C1-2702-2602-000001000000}",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Everything",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\7zFM.exe",
            r"HKLM\SOFTWARE\Classes\Installer\Products\96F071321C0420729083000010000000",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Products\96F071321C0420729083000010000000",
            r"HKCU\Software\Microsoft\Installer\Products\96F071321C0420729083000010000000",
        ] {
            assert_eq!(check_key(ok), Ok(()), "{ok}");
        }
    }

    /// Reported against Revo: tens of thousands of registry entries offered, Office among them.
    #[test]
    fn registry_system_keys_are_protected() {
        for bad in [
            r"HKLM\SOFTWARE",
            r"HKCU\Software",
            r"HKLM\SOFTWARE\WOW6432Node",
            r"HKLM\SOFTWARE\Microsoft",
            r"HKLM\SOFTWARE\Microsoft\Office",
            r"HKCU\Software\Microsoft\Office\16.0",
            r"HKLM\SOFTWARE\Microsoft\Windows",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\X\Sub",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Products",
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon",
            r"HKLM\SOFTWARE\Classes",
            r"HKLM\SOFTWARE\Classes\CLSID",
            r"HKLM\SOFTWARE\Classes\.txt",
            r"HKLM\SOFTWARE\Classes\Installer\Products",
            r"HKLM\SOFTWARE\Policies\Vendor",
            r"HKLM\SYSTEM\CurrentControlSet\Services\SomeService",
            r"HKCU\Environment",
            r"HKCU\Control Panel\Desktop",
        ] {
            assert_eq!(check_key(bad), Err(Denied::Protected), "{bad}");
        }
        for bad in ["", "HKLM", r"HKLM\", r"HKU\S-1-5-21\Software\X", r"HKCR\.txt", r"HKLM\SOFTWARE\\X"] {
            assert_eq!(check_key(bad), Err(Denied::Malformed), "{bad}");
        }
    }

    /// Keys the tracer saw 7-Zip's installer create (from a real recording), plus keys no
    /// recording may ever unlock.
    #[test]
    fn traced_keys_reach_further_but_not_to_shared_parents() {
        for ok in [
            r"HKLM\SOFTWARE\7-Zip",
            r"HKLM\SOFTWARE\Classes\CLSID\{23170F69-40C1-278A-1000-000100020000}",
            r"HKLM\SOFTWARE\Classes\WOW6432Node\CLSID\{23170F69-40C1-278A-1000-000100020000}\InprocServer32",
            r"HKLM\SOFTWARE\Classes\*\shellex\ContextMenuHandlers\7-Zip",
            r"HKLM\SOFTWARE\Classes\Directory\shellex\DragDropHandlers\7-Zip",
            r"HKLM\SOFTWARE\Classes\Directory\shell\OpenWithCode",
            r"HKLM\SOFTWARE\Classes\Directory\Background\shell\OpenWithCode\command",
            r"HKLM\SOFTWARE\Classes\.7z",
            r"HKCU\Software\Classes\7-Zip.7z\shell\open\command",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\7zFM.exe",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\7-Zip",
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\app.exe",
        ] {
            assert_eq!(check_key_traced(ok), Ok(()), "{ok}");
        }
        for bad in [
            r"HKLM\SOFTWARE",
            r"HKLM\SOFTWARE\Classes",
            r"HKLM\SOFTWARE\Classes\CLSID",
            r"HKLM\SOFTWARE\Classes\*",
            r"HKLM\SOFTWARE\Classes\.exe",
            r"HKLM\SOFTWARE\Classes\exefile",
            r"HKLM\SOFTWARE\Classes\Directory\shellex",
            r"HKLM\SOFTWARE\Classes\*\shellex",
            r"HKLM\SOFTWARE\Classes\*\shellex\ContextMenuHandlers",
            r"HKLM\SOFTWARE\Classes\Directory\Background\shellex\ContextMenuHandlers",
            r"HKLM\SOFTWARE\Classes\Directory\shell",
            r"HKLM\SOFTWARE\Classes\Directory\Background",
            r"HKLM\SOFTWARE\Classes\exefile\DefaultIcon",
            r"HKLM\SOFTWARE\Microsoft",
            r"HKLM\SOFTWARE\Microsoft\Windows",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer",
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Shell Extensions\Approved",
            r"HKLM\SOFTWARE\Policies\Vendor",
            r"HKLM\SYSTEM\CurrentControlSet\Services\X",
            r"HKCU\Environment",
        ] {
            assert_eq!(check_key_traced(bad), Err(Denied::Protected), "{bad}");
        }
    }

    #[test]
    fn only_autostart_values_may_go() {
        assert_eq!(check_value(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run"), Ok(()));
        assert_eq!(check_value(r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce"), Ok(()));
        assert_eq!(check_value(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon"), Err(Denied::Protected));
        assert_eq!(check_value(r"HKCU\Environment"), Err(Denied::Protected));
    }
}
