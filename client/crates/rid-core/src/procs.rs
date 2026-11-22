//! Running processes that belong to a program (their executable lives in its folder).

use std::io;
use std::path::Path;

use serde::Serialize;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
    TerminateProcess,
};
use windows::core::PWSTR;

use crate::safety::{norm, within};

#[derive(Serialize, Debug)]
pub struct Proc {
    pub pid: u32,
    pub path: String,
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful open call and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn image_path(pid: u32) -> Option<String> {
    // SAFETY: `buf` is valid for `len` UTF-16 units; the handle is owned by `Handle`.
    unsafe {
        let process = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn runs_in(pid: u32, dir: &str) -> Option<String> {
    let path = image_path(pid)?;
    within(&norm(Path::new(&path))?, dir).then_some(path)
}

/// Processes whose executable is inside `dir`. System processes that cannot be opened are skipped.
pub fn running_in(dir: &Path) -> Vec<Proc> {
    let Some(dir) = norm(dir) else { return Vec::new() };
    let mut out = Vec::new();
    // SAFETY: `entry.dwSize` is set as the API requires; the snapshot handle is owned by `Handle`.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let snapshot = Handle(snapshot);
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            let pid = entry.th32ProcessID;
            if pid != std::process::id()
                && let Some(path) = runs_in(pid, &dir)
            {
                out.push(Proc { pid, path });
            }
            more = Process32NextW(snapshot.0, &mut entry).is_ok();
        }
    }
    out
}

/// Every process whose executable path can be queried.
pub fn all() -> Vec<Proc> {
    let mut out = Vec::new();
    // SAFETY: `entry.dwSize` is set as the API requires; the snapshot handle is owned by `Handle`.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let snapshot = Handle(snapshot);
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            let pid = entry.th32ProcessID;
            out.extend(image_path(pid).map(|path| Proc { pid, path }));
            more = Process32NextW(snapshot.0, &mut entry).is_ok();
        }
    }
    out
}

/// Executable paths of all processes that can be queried.
pub fn all_images() -> Vec<String> {
    all().into_iter().map(|p| p.path).collect()
}

/// Terminates `pid`, but only if it (still) runs from `dir`: process ids are reused, and the
/// caller's list may be stale.
pub fn kill(pid: u32, dir: &Path) -> io::Result<()> {
    let dir = norm(dir).ok_or_else(|| io::Error::other("not a folder path"))?;
    runs_in(pid, &dir).ok_or_else(|| io::Error::other("the process does not run from this program's folder"))?;
    // SAFETY: the handle is owned by `Handle` for the duration of the call.
    unsafe {
        let process = Handle(OpenProcess(PROCESS_TERMINATE, false, pid).map_err(io::Error::other)?);
        TerminateProcess(process.0, 1).map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_refuses_by_folder() {
        let exe = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(crate::ops::system32("ping.exe"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let system32 = crate::ops::system32("");
        assert!(running_in(&system32).iter().any(|p| p.pid == child.id()));
        // The test process is never listed (Rid must not offer to kill itself), nor is ping under another folder.
        assert!(running_in(exe.parent().unwrap()).iter().all(|p| p.pid != child.id()));
        assert!(kill(child.id(), exe.parent().unwrap()).is_err());
        assert!(child.try_wait().unwrap().is_none(), "must still be running after a refused kill");
        kill(child.id(), &system32).unwrap();
        assert!(!child.wait().unwrap().success());
    }
}
