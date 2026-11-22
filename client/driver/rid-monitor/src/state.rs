//! What is being traced and the queue of recorded events.

use core::cell::UnsafeCell;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rid_proto::{EventHeader, ReadHeader};

use crate::sys;

const MAX_PIDS: usize = 256;
const QUEUE_BYTES: usize = 2 * 1024 * 1024;

/// Checked first by every callback, so an idle driver costs one load per operation.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);
pub static FOLLOW_MSIEXEC: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicU32 = AtomicU32::new(0);

/// Traced process ids; 0 marks a free slot. Read without a lock on every file and registry
/// operation, written only under the queue mutex.
static PIDS: [AtomicU32; MAX_PIDS] = [const { AtomicU32::new(0) }; MAX_PIDS];

struct Queue {
    mutex: sys::KGUARDED_MUTEX,
    read: usize,
    write: usize,
    bytes: [u8; QUEUE_BYTES],
}

struct Shared(UnsafeCell<Queue>);
// SAFETY: every access to the queue goes through `locked`, which holds its mutex.
unsafe impl Sync for Shared {}

// SAFETY: all-zero is a valid value for every field; the mutex is initialised in `init`.
static QUEUE: Shared = Shared(UnsafeCell::new(unsafe { core::mem::zeroed() }));

/// Must run once, before any callback is registered.
pub unsafe fn init() {
    // SAFETY: nothing else can touch the queue yet.
    unsafe { sys::KeInitializeGuardedMutex(&raw mut (*QUEUE.0.get()).mutex) };
}

/// Runs `f` with the queue locked. Callers are at IRQL <= APC_LEVEL, which a guarded mutex
/// requires and which keeps pageable memory (file and key names) accessible inside.
fn locked<R>(f: impl FnOnce(&mut Queue) -> R) -> R {
    // SAFETY: the mutex was initialised by `init`; the exclusive reference lives only while it is held.
    unsafe {
        let queue = QUEUE.0.get();
        sys::KeAcquireGuardedMutex(&raw mut (*queue).mutex);
        let result = f(&mut *queue);
        sys::KeReleaseGuardedMutex(&raw mut (*queue).mutex);
        result
    }
}

pub fn is_traced(pid: u32) -> bool {
    pid != 0 && PIDS.iter().any(|slot| slot.load(Ordering::Relaxed) == pid)
}

pub fn add_pid(pid: u32) {
    if pid == 0 || is_traced(pid) {
        return;
    }
    locked(|_| {
        if let Some(free) = PIDS.iter().find(|slot| slot.load(Ordering::Relaxed) == 0) {
            free.store(pid, Ordering::Relaxed);
        }
    });
}

pub fn remove_pid(pid: u32) {
    locked(|_| {
        for slot in PIDS.iter().filter(|slot| slot.load(Ordering::Relaxed) == pid) {
            slot.store(0, Ordering::Relaxed);
        }
    });
}

fn live_processes() -> u32 {
    PIDS.iter().filter(|slot| slot.load(Ordering::Relaxed) != 0).count() as u32
}

pub fn start(pid: u32, follow_msiexec: bool) {
    stop();
    locked(|q| {
        q.read = 0;
        q.write = 0;
    });
    DROPPED.store(0, Ordering::Relaxed);
    FOLLOW_MSIEXEC.store(follow_msiexec, Ordering::Relaxed);
    add_pid(pid);
    ACTIVE.store(true, Ordering::Release);
}

pub fn stop() {
    ACTIVE.store(false, Ordering::Release);
    locked(|_| {
        for slot in &PIDS {
            slot.store(0, Ordering::Relaxed);
        }
    });
}

/// Queues one event. Text that does not fit a record, or a full queue, counts as dropped.
pub fn push(kind: u8, flags: u8, pid: u32, text1: &[u16], text2: &[u16]) {
    let text_bytes = (text1.len() + text2.len()) * 2;
    let size = (size_of::<EventHeader>() + text_bytes + 7) & !7;
    if size > u16::MAX as usize {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let header = EventHeader {
        size: size as u16,
        kind,
        flags,
        pid,
        len1: text1.len() as u16,
        len2: text2.len() as u16,
        reserved: 0,
    };
    let stored = locked(|q| {
        if q.write + size > QUEUE_BYTES && q.read > 0 {
            // Reclaim what the client has already read.
            q.bytes.copy_within(q.read..q.write, 0);
            q.write -= q.read;
            q.read = 0;
        }
        let Some(slot) = q.bytes.get_mut(q.write..q.write + size) else { return false };
        slot.fill(0);
        // SAFETY: `slot` is exactly `size` bytes, which covers the header and both texts.
        unsafe {
            let at = slot.as_mut_ptr();
            core::ptr::write_unaligned(at.cast::<EventHeader>(), header);
            let text = at.add(size_of::<EventHeader>());
            core::ptr::copy_nonoverlapping(text1.as_ptr().cast::<u8>(), text, text1.len() * 2);
            core::ptr::copy_nonoverlapping(text2.as_ptr().cast::<u8>(), text.add(text1.len() * 2), text2.len() * 2);
        }
        q.write += size;
        true
    });
    if !stored {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Fills `out` with a [`ReadHeader`] and as many whole records as fit. Returns bytes written,
/// or `None` when `out` cannot even hold the header.
pub fn read(out: &mut [u8]) -> Option<usize> {
    let head = size_of::<ReadHeader>();
    let room = out.len().checked_sub(head)?;
    let copied = locked(|q| {
        let mut end = q.read;
        while end + size_of::<EventHeader>() <= q.write {
            // SAFETY: a complete header lies at `end`; records are only ever written whole.
            let size = unsafe { core::ptr::read_unaligned(q.bytes.as_ptr().add(end).cast::<EventHeader>()) }.size as usize;
            if size == 0 || end + size > q.write || end + size - q.read > room {
                break;
            }
            end += size;
        }
        let count = end - q.read;
        out[head..head + count].copy_from_slice(&q.bytes[q.read..end]);
        q.read = end;
        if q.read == q.write {
            q.read = 0;
            q.write = 0;
        }
        count
    });
    let header = ReadHeader {
        dropped: DROPPED.swap(0, Ordering::Relaxed),
        live_processes: live_processes(),
        bytes: copied as u32,
        reserved: 0,
    };
    // SAFETY: `out` holds at least `head` bytes (checked above).
    unsafe { core::ptr::write_unaligned(out.as_mut_ptr().cast::<ReadHeader>(), header) };
    Some(head + copied)
}
