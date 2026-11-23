//! A real-time ETW session over the kernel's own file, registry and process providers: what
//! Windows reports about every process, cut down to the ones being traced. No driver involved.

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_RECORD, EVENT_TRACE_CONTROL_QUERY,
    EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW, EVENT_TRACE_INDEPENDENT_SESSION_MODE, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
    EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME, ProcessTrace, StartTraceW, WNODE_FLAG_TRACED_GUID,
};
use windows::core::{GUID, HSTRING, PWSTR};

use super::fields::{self, Layout, Value};
use super::{Change, Event, Names};

const SESSION: &str = "Rid Uninstaller Trace";
const PROCESS: GUID = GUID::from_u128(0x22fb2cd6_0e7b_422b_a0c7_2fad1fd0e716);
const FILE: GUID = GUID::from_u128(0xedd08927_9cc4_4e65_b970_c2560fb5c289);
const REGISTRY: GUID = GUID::from_u128(0x70eb4f03_c1de_4f73_a051_33d13d5413bd);

/// Keywords: process start and stop; file create, new file, delete path, rename path;
/// registry close, set value, delete value, create, open and delete key.
const PROVIDERS: [(GUID, u64); 3] = [(PROCESS, 0x10), (FILE, 0x80 | 0x400 | 0x800 | 0x1000), (REGISTRY, 0x1 | 0x100 | 0x200 | 0x1000 | 0x2000 | 0x4000)];

const FILE_DIRECTORY_FILE: u64 = 0x1;
const FILE_DELETE_ON_CLOSE: u64 = 0x1000;
const REG_CREATED_NEW_KEY: u64 = 1;

fn check(result: WIN32_ERROR) -> io::Result<()> {
    if result == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(result.0 as i32)) }
}

/// EVENT_TRACE_PROPERTIES with room for the session name behind it, which is how the API takes it.
fn properties() -> Vec<u64> {
    let size = size_of::<EVENT_TRACE_PROPERTIES>() + (SESSION.len() + 1) * 2;
    let mut buf = vec![0u64; size.div_ceil(8)];
    // SAFETY: the buffer is zeroed, large enough and aligned for the structure.
    let p = unsafe { &mut *buf.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>() };
    p.Wnode.BufferSize = (buf.len() * 8) as u32;
    p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    p.Wnode.ClientContext = 1;
    // Independent: without it Windows drops an event for every session as soon as one session
    // cannot take it, and security products often run sessions over these same providers.
    p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_INDEPENDENT_SESSION_MODE;
    p.BufferSize = 256;
    p.MinimumBuffers = 32;
    p.MaximumBuffers = 256;
    p.FlushTimer = 1;
    p.LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    buf
}

/// The running session. Stopped when dropped.
pub struct Session(CONTROLTRACE_HANDLE);

impl Session {
    pub fn start() -> io::Result<Session> {
        let name = HSTRING::from(SESSION);
        let mut handle = CONTROLTRACE_HANDLE::default();
        // SAFETY: `properties()` is the buffer the API expects; `name` outlives the calls.
        unsafe {
            let mut result = StartTraceW(&mut handle, &name, properties().as_mut_ptr().cast());
            if result == ERROR_ALREADY_EXISTS {
                // Left running by an earlier trace that was killed before it could stop it.
                let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), &name, properties().as_mut_ptr().cast(), EVENT_TRACE_CONTROL_STOP);
                result = StartTraceW(&mut handle, &name, properties().as_mut_ptr().cast());
            }
            check(result)?;
            let session = Session(handle);
            for (provider, keywords) in PROVIDERS {
                // With a timeout the call returns once the provider is really on. Without one it
                // returns at once, and what the setup program does first would go unrecorded.
                check(EnableTraceEx2(handle, &provider, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, 5, keywords, 0, 10_000, None))?;
            }
            Ok(session)
        }
    }

    /// Events Windows could not deliver because this program read too slowly.
    pub fn lost(&self) -> u32 {
        let mut buf = properties();
        // SAFETY: as in `start`; the structure is read only after a successful query.
        unsafe {
            if ControlTraceW(self.0, None, buf.as_mut_ptr().cast(), EVENT_TRACE_CONTROL_QUERY) != ERROR_SUCCESS {
                return 0;
            }
            let p = &*buf.as_ptr().cast::<EVENT_TRACE_PROPERTIES>();
            p.EventsLost + p.RealTimeBuffersLost
        }
    }

    pub fn stop(&self) {
        // SAFETY: as in `start`. Stopping twice is harmless: the second call just fails.
        let _ = unsafe { ControlTraceW(self.0, None, properties().as_mut_ptr().cast(), EVENT_TRACE_CONTROL_STOP) };
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The fields of one event, by name.
struct Fields<'a> {
    layout: &'a [(String, u16)],
    values: &'a [Value],
}

impl<'a> Fields<'a> {
    fn get(&self, name: &str) -> Option<&'a Value> {
        self.values.get(self.layout.iter().position(|(n, _)| n == name)?)
    }

    fn number(&self, name: &str) -> Option<u64> {
        match self.get(name)? {
            Value::Number(n) => Some(*n),
            Value::Text(_) => None,
        }
    }

    fn text(&self, name: &str) -> Option<&'a str> {
        match self.get(name)? {
            Value::Text(t) => Some(t),
            Value::Number(_) => None,
        }
    }
}

/// What the reading thread and the caller share.
pub struct Shared {
    names: Names,
    follow_msiexec: bool,
    state: Mutex<State>,
}

struct State {
    pids: HashSet<u32>,
    /// Names of open files and keys by kernel object: renames and value writes name only the object.
    files: HashMap<u64, String>,
    keys: HashMap<u64, String>,
    layouts: HashMap<(GUID, u16, u8), Layout>,
    out: Sender<Event>,
}

impl Shared {
    pub fn new(follow_msiexec: bool, out: Sender<Event>) -> Arc<Shared> {
        let state = State { pids: HashSet::new(), files: HashMap::new(), keys: HashMap::new(), layouts: HashMap::new(), out };
        Arc::new(Shared { names: Names::new(), follow_msiexec, state: Mutex::new(state) })
    }

    pub fn trace(&self, pid: u32) {
        if let Ok(mut state) = self.state.lock() {
            state.pids.insert(pid);
        }
    }
}

/// Starts reading the session on a thread of its own, which ends when the session is stopped.
pub fn read(shared: Arc<Shared>) -> io::Result<JoinHandle<()>> {
    let mut name: Vec<u16> = SESSION.encode_utf16().chain([0]).collect();
    let mut log = EVENT_TRACE_LOGFILEW { LoggerName: PWSTR(name.as_mut_ptr()), Context: Arc::as_ptr(&shared) as *mut _, ..Default::default() };
    log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    log.Anonymous2.EventRecordCallback = Some(on_event);
    // SAFETY: `log` and `name` are valid for the call; the callback's context is kept alive by
    // the thread below, which holds `shared` until ProcessTrace has returned.
    let handle = unsafe { OpenTraceW(&mut log) };
    if handle.Value == u64::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(std::thread::spawn(move || {
        // SAFETY: the handle came from OpenTraceW above and is closed exactly once.
        unsafe {
            let _ = ProcessTrace(&[handle], None, None);
            let _ = CloseTrace(handle);
        }
        drop(shared);
    }))
}

unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record; its context is the `Shared` given to OpenTraceW.
    let (record, shared) = unsafe { (&*record, &*((*record).UserContext as *const Shared)) };
    if let Ok(mut state) = shared.state.lock() {
        state.handle(record, shared);
    }
}

impl State {
    fn handle(&mut self, record: &EVENT_RECORD, shared: &Shared) {
        let header = &record.EventHeader;
        let (provider, id, pid) = (header.ProviderId, header.EventDescriptor.Id, header.ProcessId);
        // Nearly everything Windows reports is about processes that are not traced and ends here.
        let wanted = match provider {
            PROCESS => matches!(id, 1 | 2),
            FILE => matches!(id, 12 | 26 | 27 | 30) && self.pids.contains(&pid),
            REGISTRY => matches!(id, 1 | 2 | 3 | 5 | 6 | 13) && self.pids.contains(&pid),
            _ => false,
        };
        if !wanted {
            return;
        }
        let kind = (provider, id, header.EventDescriptor.Version);
        let layout = match self.layouts.get(&kind) {
            Some(known) => known.clone(),
            None => {
                let Some(read) = fields::layout(record) else { return };
                self.layouts.insert(kind, read.clone());
                read
            }
        };
        let values = fields::values(&layout, record);
        let f = Fields { layout: &layout, values: &values };
        let change = match (provider, id) {
            (PROCESS, 1) => self.started(f.number("ProcessID"), f.number("ParentProcessID"), f.text("ImageName"), shared),
            (PROCESS, _) => f.number("ProcessID").filter(|p| self.pids.remove(&(*p as u32))).map(|p| (p as u32, Change::ProcessExited)),
            (FILE, _) => self.file(id, f.number("FileObject"), f.number("CreateOptions").unwrap_or(0), f.text("FileName").or(f.text("FilePath")), &shared.names).map(|c| (pid, c)),
            _ => self.registry(id, &f, &shared.names).map(|c| (pid, c)),
        };
        if let Some((pid, change)) = change {
            let _ = self.out.send(Event { pid, change });
        }
    }

    fn started(&mut self, pid: Option<u64>, parent: Option<u64>, image: Option<&str>, shared: &Shared) -> Option<(u32, Change)> {
        let (pid, image) = (pid? as u32, image?);
        let child = self.pids.contains(&(parent? as u32));
        // Windows Installer does the actual work in its service, which no setup program is the parent of.
        let installer = shared.follow_msiexec && image.to_lowercase().ends_with("msiexec.exe");
        if !child && !installer {
            return None;
        }
        self.pids.insert(pid);
        Some((pid, Change::ProcessStarted { image: shared.names.file(image).unwrap_or_else(|| image.to_string()), followed: !child }))
    }

    fn file(&mut self, id: u16, object: Option<u64>, options: u64, name: Option<&str>, names: &Names) -> Option<Change> {
        let (object, name) = (object?, name?);
        // Bounded: a traced process that opens files without end must not grow this without end.
        if self.files.len() > 200_000 {
            self.files.clear();
        }
        let before = self.files.insert(object, name.to_string());
        let path = names.file(name)?;
        let temporary = options & FILE_DELETE_ON_CLOSE != 0;
        match id {
            // An open that asks for deletion on close removes what it opens.
            12 => temporary.then_some(Change::Deleted { path }),
            30 => (!temporary).then_some(Change::Created { path, dir: options & FILE_DIRECTORY_FILE != 0 }),
            26 => Some(Change::Deleted { path }),
            _ => Some(Change::Renamed { from: names.file(&before?)?, to: path }),
        }
    }

    fn registry(&mut self, id: u16, f: &Fields, names: &Names) -> Option<Change> {
        let (number, text) = (|name| f.number(name), |name| f.text(name));
        let object = number("KeyObject")?;
        if id == 13 {
            self.keys.remove(&object);
            return None;
        }
        if number("Status")? != 0 {
            return None;
        }
        if matches!(id, 1 | 2) {
            // A name is either complete or relative to the key the call was made on.
            let relative = text("RelativeName")?;
            let full = match (relative.starts_with(SEP), text("BaseName").filter(|b| !b.is_empty())) {
                (true, _) => relative.to_string(),
                (false, Some(base)) => format!("{base}{SEP}{relative}"),
                (false, None) => format!("{}{SEP}{relative}", self.keys.get(&number("BaseObject")?)?),
            };
            let full = full.trim_end_matches(SEP).to_string();
            self.keys.insert(object, full.clone());
            return (id == 1 && number("Disposition")? == REG_CREATED_NEW_KEY).then(|| names.key(&full)).flatten().map(|key| Change::KeyCreated { key });
        }
        let named = text("KeyName").filter(|n| n.starts_with(SEP)).map(str::to_string);
        let key = names.key(&named.or_else(|| self.keys.get(&object).cloned())?)?;
        let name = || text("ValueName").unwrap_or_default().to_string();
        Some(match id {
            3 => Change::KeyDeleted { key },
            5 => Change::ValueSet { key, name: name() },
            _ => Change::ValueDeleted { key, name: name() },
        })
    }
}

const SEP: char = std::path::MAIN_SEPARATOR;
