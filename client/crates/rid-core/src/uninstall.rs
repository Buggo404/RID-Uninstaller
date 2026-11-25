//! Runs a program's own uninstaller and waits for everything it starts.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows::Management::Deployment::PackageManager;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
    QueryInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
};
use windows::core::{HSTRING, PWSTR};

use crate::inventory::App;

#[derive(serde::Serialize, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub exit_code: u32,
    pub cancelled: bool,
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful create call and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn active_processes(job: HANDLE) -> Option<u32> {
    let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    // SAFETY: `info` is the structure this information class fills, with its exact size.
    unsafe {
        QueryInformationJobObject(
            Some(job),
            JobObjectBasicAccountingInformation,
            (&raw mut info).cast(),
            size_of_val(&info) as u32,
            None,
        )
        .ok()?;
    }
    Some(info.ActiveProcesses)
}

/// Starts `command` exactly as written in the registry (Windows resolves unquoted paths the
/// same way "Apps & features" does) and returns once it and every process it started have
/// exited. Uninstallers routinely copy themselves elsewhere and exit, so waiting for the
/// first process alone would return too early. Setting `cancel` terminates the whole tree.
pub fn run(command: &str, cancel: &AtomicBool) -> io::Result<Outcome> {
    let mut cmd: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: `cmd` is a writable NUL-terminated buffer as CreateProcessW requires; the returned
    // handles are owned by `Handle` values below. The job is assigned before the process runs.
    unsafe {
        CreateProcessW(None, Some(PWSTR(cmd.as_mut_ptr())), None, None, false, CREATE_SUSPENDED, None, None, &startup, &mut info)
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        let (process, thread) = (Handle(info.hProcess), Handle(info.hThread));
        let job = CreateJobObjectW(None, None).ok().map(Handle);
        let in_job = job.as_ref().is_some_and(|j| AssignProcessToJobObject(j.0, process.0).is_ok());
        ResumeThread(thread.0);

        let mut cancelled = false;
        loop {
            let done = match (&job, in_job) {
                (Some(job), true) => active_processes(job.0).is_none_or(|n| n == 0),
                // Not in a job (refused by the system): the first process is all that can be watched.
                _ => WaitForSingleObject(process.0, 0) != WAIT_TIMEOUT,
            };
            if done {
                break;
            }
            if cancel.load(Ordering::Relaxed) && !cancelled {
                cancelled = true;
                match (&job, in_job) {
                    (Some(job), true) => drop(TerminateJobObject(job.0, 1)),
                    _ => drop(TerminateProcess(process.0, 1)),
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let mut exit_code = 0;
        let _ = GetExitCodeProcess(process.0, &mut exit_code);
        Ok(Outcome { exit_code, cancelled })
    }
}

fn remove_package(full_name: &str) -> io::Result<Outcome> {
    let result = PackageManager::new()
        .and_then(|pm| pm.RemovePackageAsync(&HSTRING::from(full_name)))
        .and_then(|op| op.join())
        .map_err(io::Error::other)?;
    let code = result.ExtendedErrorCode().map_err(io::Error::other)?;
    if code.is_ok() {
        Ok(Outcome { exit_code: 0, cancelled: false })
    } else {
        Err(io::Error::other(result.ErrorText().map(|t| t.to_string()).unwrap_or_else(|_| code.message())))
    }
}

/// The command that removes `app` without showing any window, when its installer technology
/// has a documented silent mode. `None` means it can only be uninstalled interactively:
/// Rid never drives another program's dialogs by simulated clicks.
pub fn quiet_command(app: &App) -> Option<String> {
    if app.id.starts_with("APPX:") {
        return Some(String::new()); // package removal has no UI to begin with
    }
    if app.id.starts_with("STEAM:") {
        return None; // Steam always asks
    }
    if app.id.starts_with("SCOOP:") || app.id.starts_with("CHOCO:") {
        return Some(app.uninstall_string.clone()); // package managers do not ask
    }
    if !app.quiet_uninstall_string.is_empty() {
        return Some(app.quiet_uninstall_string.clone());
    }
    let key_name = app.id.rsplit(':').next().unwrap_or_default();
    if app.is_msi {
        return Some(format!("MsiExec.exe /X{key_name} /qn /norestart"));
    }
    let exe = crate::cmdline::exe_of(&app.uninstall_string)?;
    let installer = crate::installer::Installer::detect(Path::new(exe), false);
    // Inno Setup registers its entries with an `_is1` suffix, which outlives a damaged file.
    installer.silent_uninstall(&app.uninstall_string, key_name.ends_with("_is1"))
}

/// Hands the removal to Steam, which asks the user itself, and waits until Steam has dropped
/// the game's record. Steam stays running afterwards, so its processes cannot be waited for.
fn steam_uninstall(app: &App, cancel: &AtomicBool) -> io::Result<Outcome> {
    let manifest = crate::sources::steam_manifest(app).ok_or_else(|| io::Error::other("not a Steam game"))?;
    let exe = crate::cmdline::exe_of(&app.uninstall_string).ok_or_else(|| io::Error::other("Steam was not found"))?;
    std::process::Command::new(exe).arg(crate::cmdline::args_of(&app.uninstall_string)).spawn()?;
    while manifest.exists() && !cancel.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(Outcome { exit_code: 0, cancelled: manifest.exists() })
}

pub fn uninstall(app: &App, quiet: bool, cancel: &AtomicBool) -> io::Result<Outcome> {
    if app.id.starts_with("STEAM:") {
        return steam_uninstall(app, cancel);
    }
    match app.id.strip_prefix("APPX:") {
        Some(full_name) => remove_package(full_name),
        None if app.uninstall_string.is_empty() => Err(io::Error::other("this program has no uninstaller")),
        None => match quiet.then(|| quiet_command(app)).flatten() {
            Some(command) => run(&command, cancel),
            None => run(&app.uninstall_string, cancel),
        },
    }
}

pub fn modify(app: &App, cancel: &AtomicBool) -> io::Result<Outcome> {
    if app.modify_path.is_empty() {
        return Err(io::Error::other("this program cannot be modified"));
    }
    run(&app.modify_path, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn cmd(script: &str) -> String {
        format!("{} /c {script}", crate::ops::system32("cmd.exe").display())
    }

    #[test]
    fn waits_for_processes_started_by_the_uninstaller() {
        // The first process returns at once; the one it started keeps running for about two seconds.
        let started = Instant::now();
        let outcome = run(&cmd("start /b ping -n 3 127.0.0.1 >nul & exit 7"), &AtomicBool::new(false)).unwrap();
        assert_eq!(outcome, Outcome { exit_code: 7, cancelled: false });
        assert!(started.elapsed() >= Duration::from_millis(1500), "returned after {:?}", started.elapsed());
    }

    #[test]
    fn cancel_terminates_the_whole_tree() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let outcome = run(&cmd("start /b ping -n 60 127.0.0.1 >nul & ping -n 60 127.0.0.1 >nul"), &cancel).unwrap();
        assert!(outcome.cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn missing_uninstaller_is_an_error() {
        assert!(run(r"C:\no\such\folder\unins000.exe /S", &AtomicBool::new(false)).is_err());
        assert!(uninstall(&App::default(), false, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn silent_switches_by_installer_technology() {
        let app = |id: &str, uninstall: &str| App { id: id.into(), uninstall_string: uninstall.into(), ..App::default() };
        let msi = App { is_msi: true, ..app("HKLM:64:{658DA597-F33D-4B4D-8960-0BFCFB80DB73}", "MsiExec.exe /X{658DA597-F33D-4B4D-8960-0BFCFB80DB73}") };
        assert_eq!(quiet_command(&msi).unwrap(), "MsiExec.exe /X{658DA597-F33D-4B4D-8960-0BFCFB80DB73} /qn /norestart");

        let inno = app("HKLM:64:LibUSB-Win32_is1", r#""D:\Program Files\LibUSB-Win32\unins000.exe""#);
        assert_eq!(quiet_command(&inno).unwrap(), r#""D:\Program Files\LibUSB-Win32\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"#);

        let declared = App { quiet_uninstall_string: r#""C:\x\u.exe" --silent"#.into(), ..app("HKCU:64:x", r#""C:\x\u.exe""#) };
        assert_eq!(quiet_command(&declared).unwrap(), r#""C:\x\u.exe" --silent"#);

        // An installer of unknown make gets no guessed switch: it must run interactively.
        let unknown = app("HKLM:64:Unknown", &crate::ops::system32("cmd.exe").to_string_lossy());
        assert_eq!(quiet_command(&unknown), None);
        assert_eq!(quiet_command(&app("HKLM:64:Gone", r"C:\no\such\uninstall.exe")), None);
    }
}
