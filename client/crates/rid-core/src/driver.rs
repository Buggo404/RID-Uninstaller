//! Installs, loads and removes the RidMonitor driver that install tracing relies on.

use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

use crate::trace::Session;

const NAME: &str = "RidMonitor";
/// Must equal ALTITUDE in the driver. Development value; a released driver gets its own from Microsoft.
const ALTITUDE: &str = "385210";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// ERROR_INVALID_IMAGE_HASH: Windows could not verify the driver's signature.
pub const UNSIGNED: i32 = 577;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Loaded and answering.
    Ready,
    /// Registered as a service but not loaded.
    Installed,
    NotInstalled,
}

fn system32(file: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join(file)
}

fn run(exe: &str, args: &[&str]) -> io::Result<String> {
    let out = Command::new(system32(exe)).args(args).creation_flags(CREATE_NO_WINDOW).output()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        return Ok(text);
    }
    // These tools print in the console's code page, which is not what the window shows text in.
    // The error number reads the same everywhere, and Windows has the message for it.
    Err(match (failure_code(&text), out.status.code()) {
        (Some(Ok(win32)), _) => io::Error::from_raw_os_error(win32),
        (Some(Err(hresult)), _) => io::Error::other(format!("{exe} {}: error {hresult}", args.join(" "))),
        // sc.exe exits with the Win32 error itself.
        (None, Some(code)) if code > 1 => io::Error::from_raw_os_error(code),
        _ => io::Error::other(format!("{exe} {} failed", args.join(" "))),
    })
}

/// The `0x........` in a tool's output: as a Win32 error when it wraps one, as written otherwise.
fn failure_code(output: &str) -> Option<Result<i32, &str>> {
    let hresult = output.match_indices("0x").find_map(|(at, _)| {
        output.get(at..at + 10).filter(|h| h[2..].bytes().all(|b| b.is_ascii_hexdigit()))
    })?;
    Some(match hresult[2..6].eq_ignore_ascii_case("8007") {
        true => i32::from_str_radix(&hresult[6..], 16).map_err(|_| hresult),
        false => Err(hresult),
    })
}

fn service_key() -> String {
    format!("SYSTEM\\CurrentControlSet\\Services\\{NAME}")
}

pub fn status() -> Status {
    if Session::open().is_ok() {
        Status::Ready
    } else if RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(service_key(), KEY_READ).is_ok() {
        Status::Installed
    } else {
        Status::NotInstalled
    }
}

/// Registers `image` (the signed driver file) as a demand-start minifilter and loads it.
pub fn install(image: &Path) -> io::Result<()> {
    if !image.is_file() {
        return Err(io::Error::other(format!("driver file not found: {}", image.display())));
    }
    fs::copy(image, system32("drivers").join(format!("{NAME}.sys")))?;
    if RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(service_key(), KEY_READ).is_err() {
        // sc.exe takes "name= value" pairs as separate arguments.
        run(
            "sc.exe",
            &[
                "create", NAME, "type=", "filesys", "start=", "demand", "error=", "normal",
                "binPath=", &format!("System32\\drivers\\{NAME}.sys"),
                "group=", "FSFilter Activity Monitor", "depend=", "FltMgr",
            ],
        )?;
    }
    // A minifilter attaches to volumes through an "instance" that carries its altitude.
    let instance = format!("{NAME} Instance");
    let (instances, _) = RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey(format!("{}\\Instances", service_key()))?;
    instances.set_value("DefaultInstance", &instance)?;
    let (default, _) = instances.create_subkey(&instance)?;
    default.set_value("Altitude", &ALTITUDE)?;
    default.set_value("Flags", &0u32)?;
    load().inspect_err(|_| {
        // A driver Windows will not load is not left registered: nothing could use it.
        let _ = remove();
    })
}

/// Loads the installed driver if it is not running.
pub fn load() -> io::Result<()> {
    if Session::open().is_ok() {
        return Ok(());
    }
    run("fltmc.exe", &["load", NAME])?;
    Session::open().map(drop)
}

pub fn remove() -> io::Result<()> {
    // Not loaded is fine; a failure to delete the service is the error worth reporting.
    let _ = run("fltmc.exe", &["unload", NAME]);
    if RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(service_key(), KEY_READ).is_ok() {
        run("sc.exe", &["delete", NAME])?;
    }
    match fs::remove_file(system32("drivers").join(format!("{NAME}.sys"))) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_number_is_read_whatever_language_surrounds_it() {
        assert_eq!(failure_code("\u{fffd}\u{fffd}ʧ\u{fffd}ܣ\u{fffd} 0x80070241\r\nWindows \u{fffd}\u{fffd}"), Some(Ok(UNSIGNED)));
        assert_eq!(failure_code("Load failed with error: 0x801f0013"), Some(Err("0x801f0013")));
        assert_eq!(failure_code("0xZZ and nothing else"), None);
        assert_eq!(failure_code(""), None);
    }
}
