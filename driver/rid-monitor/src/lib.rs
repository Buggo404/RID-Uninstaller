//! RidMonitor: a file system minifilter that records what an installer's process tree creates,
//! renames and deletes on disk and in the registry. It observes only; no operation is ever
//! blocked or altered. A user-mode client drives it through buffered IOCTLs on a control
//! device that only administrators and the system may open.
#![no_std]
#![no_main]

#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code, unused_imports, clippy::all)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

mod callbacks;
mod state;
mod text;

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU32, Ordering};

use rid_proto as proto;
use text::{unicode, w};

const STATUS_SUCCESS: i32 = 0;
const STATUS_FLT_DO_NOT_DETACH: i32 = 0xC01C_0010_u32 as i32;
const STATUS_INVALID_PARAMETER: i32 = 0xC000_000D_u32 as i32;
const STATUS_INVALID_DEVICE_REQUEST: i32 = 0xC000_0010_u32 as i32;
const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023_u32 as i32;

/// Load-order position among minifilters and registry filters ("FSFilter Activity Monitor"
/// range). Placeholder for development: a released driver needs one assigned by Microsoft.
const ALTITUDE: &[u16] = w!("385210");

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: a broken invariant in kernel mode cannot be recovered from; stop with a
    // recognisable code ("RID\0") instead of continuing on corrupt state.
    unsafe { sys::KeBugCheckEx(0xE2, 0x5249_4400, 0, 0, 0) }
}

/// Referenced by the MSVC runtime whenever floating point appears in a binary.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// The prebuilt `core` library carries unwind tables that name the MSVC C++ exception
/// handler. Nothing in a driver built with `panic = "abort"` ever unwinds, so this is never
/// called; it exists to satisfy the linker. Returns ExceptionContinueSearch.
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3(_record: *mut c_void, _frame: *mut c_void, _context: *mut c_void, _dispatch: *mut c_void) -> i32 {
    1
}

struct Registration(sys::FLT_REGISTRATION);
// SAFETY: immutable after construction; the pointers inside refer to other statics.
unsafe impl Sync for Registration {}
struct Operations([sys::FLT_OPERATION_REGISTRATION; 3]);
// SAFETY: as above.
unsafe impl Sync for Operations {}

static OPERATIONS: Operations = Operations([
    sys::FLT_OPERATION_REGISTRATION {
        MajorFunction: sys::IRP_MJ_CREATE as u8,
        Flags: 0,
        PreOperation: None,
        PostOperation: Some(callbacks::post_create),
        Reserved1: null_mut(),
    },
    sys::FLT_OPERATION_REGISTRATION {
        MajorFunction: sys::IRP_MJ_SET_INFORMATION as u8,
        Flags: 0,
        PreOperation: Some(callbacks::pre_set_information),
        PostOperation: None,
        Reserved1: null_mut(),
    },
    // IRP_MJ_OPERATION_END terminates the list.
    sys::FLT_OPERATION_REGISTRATION { MajorFunction: 0x80, Flags: 0, PreOperation: None, PostOperation: None, Reserved1: null_mut() },
]);

static REGISTRATION: Registration = Registration(sys::FLT_REGISTRATION {
    Size: size_of::<sys::FLT_REGISTRATION>() as u16,
    Version: sys::FLT_REGISTRATION_VERSION as u16,
    Flags: 0,
    ContextRegistration: core::ptr::null(),
    OperationRegistration: &raw const OPERATIONS.0 as *const sys::FLT_OPERATION_REGISTRATION,
    FilterUnloadCallback: Some(unload),
    InstanceSetupCallback: None,
    InstanceQueryTeardownCallback: Some(query_teardown),
    InstanceTeardownStartCallback: None,
    InstanceTeardownCompleteCallback: None,
    GenerateFileNameCallback: None,
    NormalizeNameComponentCallback: None,
    NormalizeContextCleanupCallback: None,
    TransactionNotificationCallback: None,
    NormalizeNameComponentExCallback: None,
    SectionNotificationCallback: None,
});

/// What `DriverEntry` set up, so that `unload` (and a failed entry) undoes exactly that.
struct Loaded {
    filter: sys::PFLT_FILTER,
    device: *mut sys::DEVICE_OBJECT,
    symlink: bool,
    registry: bool,
    process: bool,
}

static mut LOADED: Loaded = Loaded { filter: null_mut(), device: null_mut(), symlink: false, registry: false, process: false };

unsafe fn teardown() {
    state::ACTIVE.store(false, Ordering::Release);
    // SAFETY: runs single-threaded (driver entry or unload); each step undoes one successful
    // registration, in reverse order, and waits for callbacks in flight where the API does so.
    unsafe {
        let loaded = &raw mut LOADED;
        if (*loaded).process {
            let _ = sys::PsSetCreateProcessNotifyRoutineEx(Some(callbacks::process), 1);
            (*loaded).process = false;
        }
        if (*loaded).registry {
            let _ = sys::CmUnRegisterCallback(callbacks::REGISTRY_COOKIE);
            (*loaded).registry = false;
        }
        if !(*loaded).filter.is_null() {
            sys::FltUnregisterFilter((*loaded).filter);
            (*loaded).filter = null_mut();
        }
        if (*loaded).symlink {
            let _ = sys::IoDeleteSymbolicLink(&mut unicode(w!("\\DosDevices\\RidMonitor")));
            (*loaded).symlink = false;
        }
        if !(*loaded).device.is_null() {
            sys::IoDeleteDevice((*loaded).device);
            (*loaded).device = null_mut();
        }
    }
}

/// Handles open on the control device. The driver must not be unloaded while one exists:
/// its close request would be dispatched into code that is no longer there.
static OPEN_HANDLES: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn unload(flags: sys::FLT_FILTER_UNLOAD_FLAGS) -> i32 {
    let mandatory = flags & sys::FLTFL_FILTER_UNLOAD_MANDATORY != 0;
    if !mandatory && OPEN_HANDLES.load(Ordering::Acquire) != 0 {
        return STATUS_FLT_DO_NOT_DETACH;
    }
    // SAFETY: Filter Manager calls this once, when the driver is being unloaded.
    unsafe { teardown() };
    STATUS_SUCCESS
}

unsafe extern "C" fn query_teardown(_objects: *const sys::FLT_RELATED_OBJECTS, _flags: sys::FLT_INSTANCE_QUERY_TEARDOWN_FLAGS) -> i32 {
    STATUS_SUCCESS
}

unsafe fn complete(irp: *mut sys::IRP, status: i32, information: usize) -> i32 {
    // SAFETY: `irp` is the request being dispatched and is completed exactly once.
    unsafe {
        (*irp).IoStatus.__bindgen_anon_1.Status = status;
        (*irp).IoStatus.Information = information as u64;
        sys::IofCompleteRequest(irp, sys::IO_NO_INCREMENT as i8);
    }
    status
}

unsafe extern "C" fn open(_device: *mut sys::DEVICE_OBJECT, irp: *mut sys::IRP) -> i32 {
    OPEN_HANDLES.fetch_add(1, Ordering::AcqRel);
    // SAFETY: see `complete`.
    unsafe { complete(irp, STATUS_SUCCESS, 0) }
}

unsafe extern "C" fn close(_device: *mut sys::DEVICE_OBJECT, irp: *mut sys::IRP) -> i32 {
    OPEN_HANDLES.fetch_sub(1, Ordering::AcqRel);
    // SAFETY: see `complete`.
    unsafe { complete(irp, STATUS_SUCCESS, 0) }
}

/// The client closed its handle (or died): stop tracing so no events pile up unread.
unsafe extern "C" fn cleanup(_device: *mut sys::DEVICE_OBJECT, irp: *mut sys::IRP) -> i32 {
    state::stop();
    // SAFETY: see `complete`.
    unsafe { complete(irp, STATUS_SUCCESS, 0) }
}

unsafe extern "C" fn device_control(_device: *mut sys::DEVICE_OBJECT, irp: *mut sys::IRP) -> i32 {
    // SAFETY: all control codes are METHOD_BUFFERED, so `SystemBuffer` is a kernel buffer of
    // max(input, output) bytes owned by the I/O manager; user memory is never touched here.
    unsafe {
        let stack = (*irp).Tail.Overlay.__bindgen_anon_2.__bindgen_anon_1.CurrentStackLocation;
        let io = &(*stack).Parameters.DeviceIoControl;
        let (input, output) = (io.InputBufferLength as usize, io.OutputBufferLength as usize);
        let buffer = (*irp).AssociatedIrp.SystemBuffer.cast::<u8>();
        let read_u32 = |at: usize| core::ptr::read_unaligned(buffer.add(at).cast::<u32>());
        let (status, written) = match io.IoControlCode {
            _ if buffer.is_null() && (input > 0 || output > 0) => (STATUS_INVALID_PARAMETER, 0),
            proto::IOCTL_VERSION if output >= 4 => {
                core::ptr::write_unaligned(buffer.cast::<u32>(), proto::VERSION);
                (STATUS_SUCCESS, 4)
            }
            proto::IOCTL_START if input >= size_of::<proto::Start>() => {
                state::start(read_u32(0), read_u32(4) & proto::START_FOLLOW_MSIEXEC != 0);
                (STATUS_SUCCESS, 0)
            }
            proto::IOCTL_ADD_PID if input >= 4 => {
                state::add_pid(read_u32(0));
                (STATUS_SUCCESS, 0)
            }
            proto::IOCTL_STOP => {
                state::stop();
                (STATUS_SUCCESS, 0)
            }
            proto::IOCTL_READ if !buffer.is_null() => match state::read(core::slice::from_raw_parts_mut(buffer, output)) {
                Some(bytes) => (STATUS_SUCCESS, bytes),
                None => (STATUS_BUFFER_TOO_SMALL, 0),
            },
            proto::IOCTL_VERSION | proto::IOCTL_START | proto::IOCTL_ADD_PID | proto::IOCTL_READ => (STATUS_BUFFER_TOO_SMALL, 0),
            _ => (STATUS_INVALID_DEVICE_REQUEST, 0),
        };
        complete(irp, status, written)
    }
}

/// # Safety
/// Called by the I/O manager with a valid driver object, once, at PASSIVE_LEVEL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DriverEntry(driver: *mut sys::DRIVER_OBJECT, _registry_path: *mut sys::UNICODE_STRING) -> i32 {
    // SAFETY: single-threaded initialisation; every registration is recorded in LOADED so
    // that `teardown` undoes exactly what succeeded.
    unsafe {
        state::init();
        let loaded = &raw mut LOADED;

        // System and administrators only.
        let sddl = unicode(w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)"));
        let mut device = null_mut();
        let status = sys::WdmlibIoCreateDeviceSecure(
            driver,
            0,
            &mut unicode(w!("\\Device\\RidMonitor")),
            sys::FILE_DEVICE_UNKNOWN,
            sys::FILE_DEVICE_SECURE_OPEN,
            1, // exclusive: one client at a time
            &sddl,
            core::ptr::null(),
            &mut device,
        );
        if status < 0 {
            return status;
        }
        (*loaded).device = device;
        (*driver).MajorFunction[sys::IRP_MJ_CREATE as usize] = Some(open);
        (*driver).MajorFunction[sys::IRP_MJ_CLOSE as usize] = Some(close);
        (*driver).MajorFunction[sys::IRP_MJ_CLEANUP as usize] = Some(cleanup);
        (*driver).MajorFunction[sys::IRP_MJ_DEVICE_CONTROL as usize] = Some(device_control);

        let mut status =
            sys::IoCreateSymbolicLink(&mut unicode(w!("\\DosDevices\\RidMonitor")), &mut unicode(w!("\\Device\\RidMonitor")));
        (*loaded).symlink = status >= 0;
        if status >= 0 {
            status = sys::FltRegisterFilter(driver, &raw const REGISTRATION.0, &raw mut (*loaded).filter);
        }
        if status >= 0 {
            status = sys::CmRegisterCallbackEx(
                Some(callbacks::registry),
                &unicode(ALTITUDE),
                driver.cast::<c_void>(),
                null_mut(),
                &raw mut callbacks::REGISTRY_COOKIE,
                null_mut(),
            );
            (*loaded).registry = status >= 0;
        }
        if status >= 0 {
            status = sys::PsSetCreateProcessNotifyRoutineEx(Some(callbacks::process), 0);
            (*loaded).process = status >= 0;
        }
        if status >= 0 {
            status = sys::FltStartFiltering((*loaded).filter);
        }
        if status < 0 {
            teardown();
        }
        status
    }
}
