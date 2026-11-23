//! Install tracing: runs a setup program and records what it and its descendants change,
//! from the events Windows itself reports (ETW). Needs administrator rights, nothing else.

mod etw;
mod fields;
mod names;

use std::collections::HashSet;
use std::io;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde::Serialize;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW};
use windows::core::PWSTR;

pub use names::Names;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    Created { path: String, dir: bool },
    Renamed { from: String, to: String },
    Deleted { path: String },
    KeyCreated { key: String },
    KeyDeleted { key: String },
    ValueSet { key: String, name: String },
    ValueDeleted { key: String, name: String },
    /// `followed`: traced as the Windows Installer service, not as a descendant of the setup program.
    ProcessStarted { image: String, followed: bool },
    ProcessExited,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub pid: u32,
    #[serde(flatten)]
    pub change: Change,
}

#[derive(Serialize, Debug, Default)]
pub struct Recording {
    pub events: Vec<Event>,
    /// Events Windows could not deliver. Non-zero means the recording is incomplete and
    /// should not be trusted for removal.
    pub dropped: u32,
}

/// Runs `command` (a full command line) and records what it and its descendants change,
/// until all of them have exited or `cancel` is set. `on_event` sees each event as it arrives.
pub fn record(command: &str, follow_msiexec: bool, cancel: &AtomicBool, mut on_event: impl FnMut(&Event)) -> io::Result<Recording> {
    if follow_msiexec {
        let _ = std::process::Command::new(crate::ops::system32("sc.exe")).args(["stop", "msiserver"]).creation_flags(0x0800_0000).output();
    }
    let session = etw::Session::start()?;
    let (events_in, events) = mpsc::channel();
    let shared = etw::Shared::new(follow_msiexec, events_in);
    let reader = etw::read(shared.clone())?;

    let mut cmd: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: `cmd` is a writable NUL-terminated buffer; both handles are closed below.
    // The process starts suspended so that it is being traced before its first instruction.
    unsafe {
        CreateProcessW(None, Some(PWSTR(cmd.as_mut_ptr())), None, None, false, CREATE_SUSPENDED, None, None, &startup, &mut info)
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
    }
    shared.trace(info.dwProcessId);
    if follow_msiexec {
        // The Windows Installer service may still be idling from an earlier installation. Keys it
        // opened back then cannot be named now, so an idle one is asked to stop and a fresh one
        // starts with the installation; one that is busy refuses and is traced as it is.
        for service in crate::procs::all().iter().filter(|p| p.path.to_lowercase().ends_with("\\msiexec.exe")) {
            shared.trace(service.pid);
        }
    }
    // SAFETY: handles returned by CreateProcessW above.
    unsafe {
        ResumeThread(info.hThread);
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }

    let mut recording = Recording::default();
    // The setup program and its descendants. Setup is over when none of them is left; the
    // Windows Installer service is not among them, as it lingers for minutes.
    let mut setup: HashSet<u32> = HashSet::from([info.dwProcessId]);
    let mut take = |event: Event, setup: &mut HashSet<u32>, recording: &mut Recording| {
        match event.change {
            Change::ProcessStarted { followed: false, .. } => drop(setup.insert(event.pid)),
            Change::ProcessExited => drop(setup.remove(&event.pid)),
            _ => {}
        }
        on_event(&event);
        recording.events.push(event);
    };
    while !setup.is_empty() && !cancel.load(Ordering::Relaxed) {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => take(event, &mut setup, &mut recording),
            // With events lost, the exit of a process may be among them: ask Windows who is left.
            Err(RecvTimeoutError::Timeout) if session.lost() > 0 => {
                let alive: HashSet<u32> = crate::procs::all().iter().map(|p| p.pid).collect();
                setup.retain(|pid| alive.contains(pid));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    recording.dropped = session.lost();
    // Stopping makes Windows hand over what it still holds; the reader ends once it has it all.
    session.stop();
    let _ = reader.join();
    for event in events.try_iter() {
        take(event, &mut setup, &mut recording);
    }
    Ok(recording)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs administrator rights, as tracing does; without them there is nothing to test.
    #[test]
    fn what_a_command_creates_renames_and_deletes_is_recorded() {
        // Windows reports full names; the temporary folder may be given in its short 8.3 form.
        let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let dir = std::path::PathBuf::from(temp.to_string_lossy().trim_start_matches(r"\\?\")).join(format!("rid-trace-{}", std::process::id()));
        let key = format!("RidTraceTest{}", std::process::id());
        let (d, k) = (dir.display(), format!("HKCU\\Software\\{key}"));
        let script = format!(
            "cmd.exe /c mkdir \"{d}\\sub\" & echo x> \"{d}\\a.tmp\" & ren \"{d}\\a.tmp\" a.txt & echo y> \"{d}\\gone.txt\" & del \"{d}\\gone.txt\" \
             & reg add {k}\\Child /v Name /d 1 /f & reg delete {k}\\Child /f & reg add {k} /v Kept /d 1 /f"
        );
        let recording = match record(&script, false, &AtomicBool::new(false), |_| {}) {
            Err(e) if e.raw_os_error() == Some(5) => return eprintln!("skipped: not running as administrator"),
            other => other.unwrap(),
        };
        let items = crate::traced::reduce(&recording.events);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(format!("Software\\{key}"));

        let changes: Vec<&Change> = recording.events.iter().map(|e| &e.change).collect();
        let has = |wanted: Change| assert!(changes.contains(&&wanted), "missing {wanted:?} in {changes:#?}");
        assert_eq!(recording.dropped, 0);
        has(Change::Created { path: format!("{d}"), dir: true });
        has(Change::Created { path: format!("{d}\\sub"), dir: true });
        has(Change::Created { path: format!("{d}\\a.tmp"), dir: false });
        has(Change::Renamed { from: format!("{d}\\a.tmp"), to: format!("{d}\\a.txt") });
        has(Change::Deleted { path: format!("{d}\\gone.txt") });
        has(Change::KeyCreated { key: k.clone() });
        has(Change::KeyCreated { key: format!("{k}\\Child") });
        has(Change::KeyDeleted { key: format!("{k}\\Child") });
        has(Change::ValueSet { key: k.clone(), name: "Kept".into() });
        assert!(changes.iter().any(|c| matches!(c, Change::ProcessStarted { image, followed: false } if image.to_lowercase().ends_with("reg.exe"))), "{changes:#?}");
        // The net result: the folder and the key, nothing that came and went.
        use crate::scan::Target;
        assert_eq!(items, [Target::Dir { path: format!("{d}") }, Target::RegKey { key: k }], "{items:#?}");
    }
}
