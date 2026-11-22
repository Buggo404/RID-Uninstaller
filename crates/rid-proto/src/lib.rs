//! The contract between the RidMonitor kernel driver and its user-mode client.
//! Both sides compile this file, so the layouts cannot drift apart.

#![no_std]

/// Bumped whenever a control code or a layout below changes.
pub const VERSION: u32 = 2;

/// Win32 path of the control device.
pub const DEVICE_WIN32: &str = r"\\.\RidMonitor";

const fn ctl(function: u32) -> u32 {
    // CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800 + function, METHOD_BUFFERED, FILE_ANY_ACCESS)
    (0x22 << 16) | ((0x800 + function) << 2)
}

/// Out: `u32` protocol version.
pub const IOCTL_VERSION: u32 = ctl(0);
/// In: [`Start`]. Clears earlier events and starts tracing the given process and its descendants.
pub const IOCTL_START: u32 = ctl(1);
/// In: `u32` process id to trace as well (for helpers that are not descendants).
pub const IOCTL_ADD_PID: u32 = ctl(2);
/// Stops tracing. Queued events stay readable.
pub const IOCTL_STOP: u32 = ctl(3);
/// Out: [`ReadHeader`] followed by as many whole event records as fit.
pub const IOCTL_READ: u32 = ctl(4);

/// Also trace every `msiexec.exe` that starts: Windows Installer does the actual work in its
/// service process, which is not a descendant of the setup program.
pub const START_FOLLOW_MSIEXEC: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Start {
    pub pid: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ReadHeader {
    /// Events lost since the last read because the queue was full.
    pub dropped: u32,
    /// Traced processes still running. Zero means the installation has finished.
    pub live_processes: u32,
    /// Bytes of event records following this header.
    pub bytes: u32,
    pub reserved: u32,
}

/// Header of one event. Followed by `len1` then `len2` UTF-16 code units, then padding to
/// a multiple of 8 bytes; `size` covers all of it.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct EventHeader {
    pub size: u16,
    pub kind: u8,
    /// [`FLAG_DIRECTORY`]
    pub flags: u8,
    pub pid: u32,
    pub len1: u16,
    pub len2: u16,
    pub reserved: u32,
}

pub const FLAG_DIRECTORY: u8 = 1;
/// On [`EVENT_KEY_CREATE`]: the driver could not tell a new key from an existing one being opened.
pub const FLAG_UNSURE: u8 = 2;
/// On [`EVENT_PROCESS_START`]: traced because of [`START_FOLLOW_MSIEXEC`], not as a descendant
/// of the setup program. Such a process may outlive the installation (the Windows Installer
/// service idles for minutes), so it says nothing about whether setup has finished.
pub const FLAG_FOLLOWED: u8 = 4;

/// A file or folder was created. Text 1: NT path (`\Device\HarddiskVolume3\...`).
pub const EVENT_CREATE: u8 = 1;
/// Rename requested. Text 1: old NT path, text 2: new NT path.
pub const EVENT_RENAME: u8 = 2;
/// Deletion requested. Text 1: NT path.
pub const EVENT_DELETE: u8 = 3;
/// A registry key was created. Text 1: kernel key path (`\REGISTRY\MACHINE\...`).
pub const EVENT_KEY_CREATE: u8 = 4;
/// Deletion of a key requested. Text 1: kernel key path.
pub const EVENT_KEY_DELETE: u8 = 5;
/// A value was written. Text 1: kernel key path, text 2: value name.
pub const EVENT_VALUE_SET: u8 = 6;
/// Deletion of a value requested. Text 1: kernel key path, text 2: value name.
pub const EVENT_VALUE_DELETE: u8 = 7;
/// A traced process started. Text 1: image path.
pub const EVENT_PROCESS_START: u8 = 8;
pub const EVENT_PROCESS_EXIT: u8 = 9;
