//! Install tracing: talks to the RidMonitor driver and turns its event records into changes.

mod names;

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rid_proto::{self as proto, EventHeader, ReadHeader};
use serde::Serialize;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Threading::{CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW};
use windows::core::PWSTR;

pub use names::Names;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    Created { path: String, dir: bool },
    Renamed { from: String, to: String, dir: bool },
    Deleted { path: String, dir: bool },
    /// `unsure`: the driver could not tell a newly created key from an existing one being opened.
    KeyCreated { key: String, unsure: bool },
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

/// An open connection to the driver. Tracing stops when it is dropped.
pub struct Session(File);

impl Session {
    pub fn open() -> io::Result<Session> {
        let device = OpenOptions::new().read(true).write(true).open(proto::DEVICE_WIN32)?;
        let session = Session(device);
        let mut version = [0u8; 4];
        session.control(proto::IOCTL_VERSION, &[], &mut version)?;
        match u32::from_le_bytes(version) {
            proto::VERSION => Ok(session),
            other => Err(io::Error::other(format!("driver speaks protocol {other}, this program {}", proto::VERSION))),
        }
    }

    fn control(&self, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
        let mut returned = 0u32;
        // SAFETY: the buffers are valid for the lengths passed; the handle is open for `self`.
        unsafe {
            DeviceIoControl(
                HANDLE(self.0.as_raw_handle()),
                code,
                (!input.is_empty()).then_some(input.as_ptr().cast()),
                input.len() as u32,
                (!output.is_empty()).then_some(output.as_mut_ptr().cast()),
                output.len() as u32,
                Some(&mut returned),
                None,
            )
        }
        .map_err(io::Error::other)?;
        Ok(returned as usize)
    }

    pub fn start(&self, pid: u32, follow_msiexec: bool) -> io::Result<()> {
        let flags = if follow_msiexec { proto::START_FOLLOW_MSIEXEC } else { 0 };
        let input = [pid.to_le_bytes(), flags.to_le_bytes()].concat();
        self.control(proto::IOCTL_START, &input, &mut []).map(drop)
    }

    pub fn add_pid(&self, pid: u32) -> io::Result<()> {
        self.control(proto::IOCTL_ADD_PID, &pid.to_le_bytes(), &mut []).map(drop)
    }

    pub fn stop(&self) -> io::Result<()> {
        self.control(proto::IOCTL_STOP, &[], &mut []).map(drop)
    }

    /// Queued events, plus the driver's counters (events dropped, traced processes still alive).
    pub fn read(&self, names: &Names) -> io::Result<(ReadHeader, Vec<Event>)> {
        let mut buf = vec![0u8; 256 * 1024];
        let got = self.control(proto::IOCTL_READ, &[], &mut buf)?;
        if got < size_of::<ReadHeader>() {
            return Err(io::Error::other("short reply from the driver"));
        }
        // SAFETY: the buffer holds at least a header, which is plain data.
        let header: ReadHeader = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
        let records = &buf[size_of::<ReadHeader>()..got];
        Ok((header, parse(records, names)))
    }
}

/// Decodes the records of one read. Stops at the first malformed record rather than guessing.
fn parse(mut records: &[u8], names: &Names) -> Vec<Event> {
    let mut events = Vec::new();
    while records.len() >= size_of::<EventHeader>() {
        // SAFETY: at least a header's worth of bytes remains; the header is plain data.
        let h: EventHeader = unsafe { std::ptr::read_unaligned(records.as_ptr().cast()) };
        let (size, len1, len2) = (h.size as usize, h.len1 as usize * 2, h.len2 as usize * 2);
        let text_at = size_of::<EventHeader>();
        if size < text_at + len1 + len2 || size > records.len() {
            break;
        }
        let wide = |bytes: &[u8]| {
            String::from_utf16_lossy(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
        };
        let text1 = wide(&records[text_at..text_at + len1]);
        let text2 = wide(&records[text_at + len1..text_at + len1 + len2]);
        let dir = h.flags & proto::FLAG_DIRECTORY != 0;
        // Paths that are not on a drive letter, and other users' hives, are not this program's concern.
        let change = match h.kind {
            proto::EVENT_CREATE => names.file(&text1).map(|path| Change::Created { path, dir }),
            proto::EVENT_RENAME => names.file(&text1).zip(names.file(&text2)).map(|(from, to)| Change::Renamed { from, to, dir }),
            proto::EVENT_DELETE => names.file(&text1).map(|path| Change::Deleted { path, dir }),
            proto::EVENT_KEY_CREATE => {
                names.key(&text1).map(|key| Change::KeyCreated { key, unsure: h.flags & proto::FLAG_UNSURE != 0 })
            }
            proto::EVENT_KEY_DELETE => names.key(&text1).map(|key| Change::KeyDeleted { key }),
            proto::EVENT_VALUE_SET => names.key(&text1).map(|key| Change::ValueSet { key, name: text2 }),
            proto::EVENT_VALUE_DELETE => names.key(&text1).map(|key| Change::ValueDeleted { key, name: text2 }),
            proto::EVENT_PROCESS_START => Some(Change::ProcessStarted {
                image: names.file(&text1).unwrap_or(text1),
                followed: h.flags & proto::FLAG_FOLLOWED != 0,
            }),
            proto::EVENT_PROCESS_EXIT => Some(Change::ProcessExited),
            _ => None,
        };
        events.extend(change.map(|change| Event { pid: h.pid, change }));
        records = &records[size..];
    }
    events
}

#[derive(Serialize, Debug, Default)]
pub struct Recording {
    pub events: Vec<Event>,
    /// Events the driver had to drop because its queue was full. Non-zero means the
    /// recording is incomplete and should not be trusted for removal.
    pub dropped: u32,
}

/// Runs `command` (a full command line) and records what it and its descendants change,
/// until all of them have exited or `cancel` is set. `on_event` sees each event as it arrives.
pub fn record(command: &str, follow_msiexec: bool, cancel: &AtomicBool, mut on_event: impl FnMut(&Event)) -> io::Result<Recording> {
    let session = Session::open()?;
    let names = Names::new();

    let mut cmd: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: `cmd` is a writable NUL-terminated buffer; both handles are closed below.
    // The process starts suspended so that tracing is in place before its first instruction.
    unsafe {
        CreateProcessW(None, Some(PWSTR(cmd.as_mut_ptr())), None, None, false, CREATE_SUSPENDED, None, None, &startup, &mut info)
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
    }
    let mut started = session.start(info.dwProcessId, follow_msiexec);
    if started.is_ok() && follow_msiexec {
        // The Windows Installer service may still be running from an earlier installation; the
        // driver only learns about processes that start after tracing began.
        for service in crate::procs::all().iter().filter(|p| p.pid != info.dwProcessId && p.path.to_lowercase().ends_with("\\msiexec.exe")) {
            started = started.and(session.add_pid(service.pid));
        }
    }
    // SAFETY: handles returned by CreateProcessW above. Resumed even if tracing failed to
    // start, so that no suspended process is left behind.
    unsafe {
        ResumeThread(info.hThread);
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }
    started?;

    let mut recording = Recording::default();
    // The setup program and its descendants. Setup is over when none of them is left; the
    // driver's own count would also include the Windows Installer service, which lingers.
    let mut setup: HashSet<u32> = HashSet::from([info.dwProcessId]);
    let mut stopping = false;
    loop {
        if cancel.load(Ordering::Relaxed) && !stopping {
            // No new events from here on; the loop still drains what is already queued.
            session.stop()?;
            stopping = true;
        }
        let (header, events) = session.read(&names)?;
        recording.dropped += header.dropped;
        for event in &events {
            match event.change {
                Change::ProcessStarted { followed: false, .. } => drop(setup.insert(event.pid)),
                Change::ProcessExited => drop(setup.remove(&event.pid)),
                _ => {}
            }
            on_event(event);
        }
        let idle = events.is_empty();
        recording.events.extend(events);
        if (setup.is_empty() || header.live_processes == 0 || stopping) && idle {
            break;
        }
        if idle {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    session.stop()?;
    Ok(recording)
}
