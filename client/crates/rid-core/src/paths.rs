//! Where Rid keeps its own files, and small filesystem helpers.

use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Preferences. A file named `portable` next to the executable keeps everything beside it
/// instead of in the user profile.
pub fn data_dir() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(dir) = exe_dir.filter(|d| d.join("portable").is_file()) {
        return dir.join("data");
    }
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Rid Uninstaller")
}

/// Removes everyone but administrators and the system from the writers of `dir`.
///
/// Rid runs elevated and later acts on what it finds in its own folders (it restores files,
/// imports registry exports, deletes traced items). If an unprivileged process could write
/// there, it could have Rid do those things on its behalf.
pub fn lock_down(dir: &Path) -> io::Result<()> {
    let icacls = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("icacls.exe");
    // Well-known SIDs, so this works on every display language: Administrators, SYSTEM, Users.
    let admins = "*S-1-5-32-544";
    let steps: [&[&str]; 2] = [
        &["/setowner", admins, "/T", "/C", "/Q"],
        &["/inheritance:r", "/grant:r", "*S-1-5-32-544:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-545:(OI)(CI)RX", "/T", "/C", "/Q"],
    ];
    for args in steps {
        let out = Command::new(&icacls).arg(dir).args(args).creation_flags(0x0800_0000).output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "could not restrict access to {} (administrator rights are required): {}",
                dir.display(),
                String::from_utf8_lossy(&out.stdout).trim()
            )));
        }
    }
    Ok(())
}

/// Where Rid keeps what it acts on with administrator rights: quarantine records and traced
/// installations. Writable by administrators only. In portable mode this is the data folder
/// beside the executable, whose protection is then up to wherever the user put it.
pub fn secure_dir() -> io::Result<PathBuf> {
    static DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    DIR.get_or_init(|| {
        let portable = data_dir();
        if portable.file_name().is_some_and(|n| n == "data") {
            fs::create_dir_all(&portable).map_err(|e| e.to_string())?;
            return Ok(portable);
        }
        let dir = PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData is not set")?).join("Rid Uninstaller");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Every start, not only on creation: the folder may have been planted before Rid first ran.
        lock_down(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    })
    .clone()
    .map_err(io::Error::other)
}

/// Total size of the files under `dir`, in KB. Junctions and symlinks are not followed,
/// so a link to another volume or back to a parent is neither counted nor looped on.
pub fn dir_size_kb(dir: &Path) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = fs::read_dir(dir) else { return 0 };
        entries
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_symlink() => 0,
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    walk(dir).div_ceil(1024)
}
