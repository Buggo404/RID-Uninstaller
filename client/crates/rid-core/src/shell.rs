//! "Jump to" actions: every target handed to the shell is validated here first,
//! because the paths and URLs come from registry values any installer can write.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// `start` goes through ShellExecute, which shows the UAC prompt when the target needs one.
fn start(program: &str, args: &[&str]) -> io::Result<()> {
    Command::new("cmd")
        .args(["/c", "start", "", program])
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(drop)
}

pub fn open_folder(path: &str) -> io::Result<()> {
    if !Path::new(path).is_dir() {
        return Err(invalid(format!("not a folder: {path}")));
    }
    Command::new("explorer.exe").arg(path).spawn().map(drop)
}

pub fn open_url(url: &str) -> io::Result<()> {
    // Anything else (a file path, a custom scheme) would be executed by the shell.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(invalid(format!("not a web address: {url}")));
    }
    Command::new("explorer.exe").arg(url).spawn().map(drop)
}

/// Opens Registry Editor positioned on `key_path` (`HKEY_LOCAL_MACHINE\...`).
pub fn open_registry(key_path: &str) -> io::Result<()> {
    if !key_path.starts_with("HKEY_") {
        return Err(invalid(format!("not a registry path: {key_path}")));
    }
    let (regedit, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit")?;
    regedit.set_value("LastKey", &format!("Computer\\{key_path}"))?;
    start("regedit.exe", &["/m"])
}

const VERBS: [(&str, &str, &str); 3] = [
    // (file class, verb key, argument that precedes the file path)
    ("exefile", "RidTrace", "--trace"),
    ("Msi.Package", "RidTrace", "--trace"),
    ("lnkfile", "RidUninstall", "--uninstall-shortcut"),
];

fn verb_key(class: &str, verb: &str) -> String {
    format!("SOFTWARE\\Classes\\{class}\\shell\\{verb}")
}

/// Is Rid in Explorer's right-click menu for setup files and shortcuts?
pub fn context_menu_enabled() -> bool {
    VERBS.iter().all(|(class, verb, _)| RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(verb_key(class, verb)).is_ok())
}

/// Adds or removes "Install and trace" on .exe/.msi files and "Uninstall" on shortcuts.
/// Plain registry verbs: Explorer starts `app` itself, no code of Rid's runs inside Explorer.
pub fn set_context_menu(enabled: bool, app: &Path, trace_label: &str, uninstall_label: &str) -> io::Result<()> {
    let classes = RegKey::predef(HKEY_LOCAL_MACHINE);
    for (class, verb, argument) in VERBS {
        let path = verb_key(class, verb);
        if !enabled {
            // Exactly the keys created below, by their fixed names.
            match classes.delete_subkey_all(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => continue,
            }
        }
        let (key, _) = classes.create_subkey(&path)?;
        key.set_value("", &if verb == "RidTrace" { trace_label } else { uninstall_label })?;
        key.set_value("Icon", &format!("\"{}\"", app.display()))?;
        let (command, _) = key.create_subkey("command")?;
        command.set_value("", &format!("\"{}\" {argument} \"%1\"", app.display()))?;
    }
    Ok(())
}

/// The fixed set of Windows tools offered in the Tools menu.
pub fn system_tool(name: &str) -> io::Result<()> {
    let (program, args): (&str, &[&str]) = match name {
        "programs" => ("control.exe", &["appwiz.cpl"]),
        "features" => ("OptionalFeatures.exe", &[]),
        "restore" => ("rstrui.exe", &[]),
        "security" => ("windowsdefender:", &[]),
        "sysprops" => ("control.exe", &["sysdm.cpl"]),
        "sysinfo" => ("msinfo32.exe", &[]),
        "defrag" => ("dfrgui.exe", &[]),
        "services" => ("mmc.exe", &["services.msc"]),
        "tasks" => ("mmc.exe", &["taskschd.msc"]),
        _ => return Err(invalid(format!("unknown tool: {name}"))),
    };
    start(program, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_targets_the_shell_would_execute() {
        assert!(open_url(r"C:\Windows\System32\calc.exe").is_err());
        assert!(open_url("file:///C:/Windows/System32/calc.exe").is_err());
        assert!(open_url("ms-settings:").is_err());
        assert!(open_folder(r"C:\Windows\System32\calc.exe").is_err());
        assert!(open_registry(r"C:\x").is_err());
        assert!(system_tool("cmd.exe").is_err());
    }
}

/// The text on the clipboard; empty when it holds none.
pub fn clipboard_text() -> String {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: the clipboard is open while its data is read; the text block is NUL-terminated
    // UTF-16 owned by the clipboard and is only read while locked.
    unsafe {
        if OpenClipboard(None).is_err() {
            return String::new();
        }
        let text = GetClipboardData(CF_UNICODETEXT)
            .ok()
            .map(|handle| {
                let block = HGLOBAL(handle.0);
                let data = GlobalLock(block) as *const u16;
                if data.is_null() {
                    return String::new();
                }
                let len = (0..).take_while(|&i| *data.add(i) != 0).count();
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(data, len));
                let _ = GlobalUnlock(block);
                text
            })
            .unwrap_or_default();
        let _ = CloseClipboard();
        text
    }
}
