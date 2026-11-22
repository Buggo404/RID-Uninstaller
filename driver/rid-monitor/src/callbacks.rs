//! File system, registry and process callbacks. They only record; nothing is ever blocked,
//! and nothing waits on user mode.

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::Ordering;

use rid_proto as proto;

use crate::state::{self, ACTIVE, FOLLOW_MSIEXEC};
use crate::{sys, text};

const NAME_OPTIONS: u32 = sys::FLT_FILE_NAME_NORMALIZED | sys::FLT_FILE_NAME_QUERY_DEFAULT;
const STATUS_REPARSE: i32 = 0x104;

fn current_pid() -> u32 {
    // SAFETY: always callable; returns the id of the calling process.
    unsafe { sys::PsGetCurrentProcessId() as usize as u32 }
}

/// Runs `f` with the normalized NT path of the file the operation targets.
unsafe fn with_file_name(data: *mut sys::FLT_CALLBACK_DATA, f: impl FnOnce(&[u16])) {
    let mut info: *mut sys::FLT_FILE_NAME_INFORMATION = null_mut();
    // SAFETY: `data` is the callback's own argument; the name information is released below.
    unsafe {
        if sys::FltGetFileNameInformation(data, NAME_OPTIONS, &mut info) >= 0 && !info.is_null() {
            f(text::slice(&(*info).Name));
            sys::FltReleaseFileNameInformation(info);
        }
    }
}

pub unsafe extern "C" fn post_create(
    data: *mut sys::FLT_CALLBACK_DATA,
    _objects: *const sys::FLT_RELATED_OBJECTS,
    _context: *mut c_void,
    flags: sys::FLT_POST_OPERATION_FLAGS,
) -> sys::FLT_POSTOP_CALLBACK_STATUS {
    // SAFETY: Filter Manager passes valid pointers; post-create runs at PASSIVE_LEVEL.
    unsafe {
        if !ACTIVE.load(Ordering::Acquire) || flags & sys::FLTFL_POST_OPERATION_DRAINING != 0 {
            return sys::FLT_POSTOP_FINISHED_PROCESSING;
        }
        let status = (*data).IoStatus.__bindgen_anon_1.Status;
        if status < 0 || status == STATUS_REPARSE {
            return sys::FLT_POSTOP_FINISHED_PROCESSING;
        }
        let options = (*(*data).Iopb).Parameters.Create.Options;
        let created = (*data).IoStatus.Information == sys::FILE_CREATED as u64;
        let delete_on_close = options & sys::FILE_DELETE_ON_CLOSE != 0;
        let pid = sys::FltGetRequestorProcessId(data);
        if (created || delete_on_close) && state::is_traced(pid) {
            let dir = if options & sys::FILE_DIRECTORY_FILE != 0 { proto::FLAG_DIRECTORY } else { 0 };
            with_file_name(data, |name| {
                if created {
                    state::push(proto::EVENT_CREATE, dir, pid, name, &[]);
                }
                if delete_on_close {
                    state::push(proto::EVENT_DELETE, dir, pid, name, &[]);
                }
            });
        }
        sys::FLT_POSTOP_FINISHED_PROCESSING
    }
}

pub unsafe extern "C" fn pre_set_information(
    data: *mut sys::FLT_CALLBACK_DATA,
    objects: *const sys::FLT_RELATED_OBJECTS,
    _context: *mut *mut c_void,
) -> sys::FLT_PREOP_CALLBACK_STATUS {
    // SAFETY: Filter Manager passes valid pointers; set-information buffers are kernel copies.
    unsafe {
        if !ACTIVE.load(Ordering::Acquire) {
            return sys::FLT_PREOP_SUCCESS_NO_CALLBACK;
        }
        let params = &(*(*data).Iopb).Parameters.SetFileInformation;
        let class = params.FileInformationClass;
        let rename = class == sys::FileRenameInformation || class == sys::FileRenameInformationEx;
        let delete = match class {
            c if c == sys::FileDispositionInformation && params.Length >= 1 => *params.InfoBuffer.cast::<u8>() != 0,
            c if c == sys::FileDispositionInformationEx && params.Length >= 4 => {
                *params.InfoBuffer.cast::<u32>() & sys::FILE_DISPOSITION_DELETE != 0
            }
            _ => false,
        };
        let pid = sys::FltGetRequestorProcessId(data);
        if !(rename || delete) || !state::is_traced(pid) {
            return sys::FLT_PREOP_SUCCESS_NO_CALLBACK;
        }
        let mut is_dir = 0u8;
        let _ = sys::FltIsDirectory((*objects).FileObject, (*objects).Instance, &mut is_dir);
        let dir = if is_dir != 0 { proto::FLAG_DIRECTORY } else { 0 };
        with_file_name(data, |name| {
            if delete {
                state::push(proto::EVENT_DELETE, dir, pid, name, &[]);
                return;
            }
            let info = params.InfoBuffer.cast::<sys::FILE_RENAME_INFORMATION>();
            if (params.Length as usize) < core::mem::size_of::<sys::FILE_RENAME_INFORMATION>() {
                return;
            }
            let mut target: *mut sys::FLT_FILE_NAME_INFORMATION = null_mut();
            let found = sys::FltGetDestinationFileNameInformation(
                (*objects).Instance,
                (*objects).FileObject,
                (*info).RootDirectory,
                (&raw mut (*info).FileName).cast(),
                (*info).FileNameLength,
                NAME_OPTIONS,
                &mut target,
            );
            if found >= 0 && !target.is_null() {
                state::push(proto::EVENT_RENAME, dir, pid, name, text::slice(&(*target).Name));
                sys::FltReleaseFileNameInformation(target);
            }
        });
        sys::FLT_PREOP_SUCCESS_NO_CALLBACK
    }
}

/// Cookie from `CmRegisterCallbackEx`, needed to ask the registry for key names.
pub static mut REGISTRY_COOKIE: sys::LARGE_INTEGER = unsafe { core::mem::zeroed() };

/// Runs `f` with the full kernel path (`\REGISTRY\MACHINE\...`) of a key object.
unsafe fn with_key_name(key: *mut c_void, f: impl FnOnce(&[u16])) {
    let mut name: *const sys::UNICODE_STRING = core::ptr::null();
    // SAFETY: `key` is the key object handed to the registry callback; the name is released below.
    unsafe {
        let cookie = &raw mut REGISTRY_COOKIE;
        if !key.is_null() && sys::CmCallbackGetKeyObjectIDEx(cookie, key, null_mut(), &mut name, 0) >= 0 && !name.is_null() {
            f(text::slice(&*name));
            sys::CmCallbackReleaseKeyObjectIDEx(name);
        }
    }
}

pub unsafe extern "C" fn registry(_context: *mut c_void, class: *mut c_void, info: *mut c_void) -> i32 {
    if !ACTIVE.load(Ordering::Acquire) || info.is_null() {
        return 0;
    }
    let pid = current_pid();
    if !state::is_traced(pid) {
        return 0;
    }
    let class = class as usize as sys::REG_NOTIFY_CLASS;
    // SAFETY: `info` has the type the notification class documents. Pointers inside it that
    // may refer to the caller's memory are only read after `text::kernel` accepts them.
    unsafe {
        if class == sys::RegNtPostCreateKeyEx {
            let post = &*info.cast::<sys::REG_POST_OPERATION_INFORMATION>();
            let pre = post.PreInformation.cast::<sys::REG_CREATE_KEY_INFORMATION>();
            if post.Status == 0 && !pre.is_null() {
                let disposition = (*pre).Disposition;
                let (created, flags) = match text::kernel(disposition.cast()) {
                    true => (*disposition == sys::REG_CREATED_NEW_KEY, 0),
                    false => (true, proto::FLAG_UNSURE),
                };
                if created {
                    with_key_name(post.Object, |key| state::push(proto::EVENT_KEY_CREATE, flags, pid, key, &[]));
                }
            }
        } else if class == sys::RegNtPreSetValueKey {
            let set = &*info.cast::<sys::REG_SET_VALUE_KEY_INFORMATION>();
            let value = text::checked(set.ValueName);
            with_key_name(set.Object, |key| state::push(proto::EVENT_VALUE_SET, 0, pid, key, value));
        } else if class == sys::RegNtPreDeleteValueKey {
            let del = &*info.cast::<sys::REG_DELETE_VALUE_KEY_INFORMATION>();
            let value = text::checked(del.ValueName);
            with_key_name(del.Object, |key| state::push(proto::EVENT_VALUE_DELETE, 0, pid, key, value));
        } else if class == sys::RegNtPreDeleteKey {
            let del = &*info.cast::<sys::REG_DELETE_KEY_INFORMATION>();
            with_key_name(del.Object, |key| state::push(proto::EVENT_KEY_DELETE, 0, pid, key, &[]));
        }
    }
    0
}

pub unsafe extern "C" fn process(_process: sys::PEPROCESS, pid: sys::HANDLE, info: *mut sys::PS_CREATE_NOTIFY_INFO) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let pid = pid as usize as u32;
    if info.is_null() {
        if state::is_traced(pid) {
            state::remove_pid(pid);
            state::push(proto::EVENT_PROCESS_EXIT, 0, pid, &[], &[]);
        }
        return;
    }
    // SAFETY: `info` is valid for the duration of the callback; the image name is kernel memory.
    unsafe {
        let image = if (*info).ImageFileName.is_null() { &[][..] } else { text::slice(&*(*info).ImageFileName) };
        // The creator is usually the parent, but a process can be started on behalf of another.
        let from_traced = state::is_traced((*info).ParentProcessId as usize as u32) || state::is_traced(current_pid());
        let installer_service = FOLLOW_MSIEXEC.load(Ordering::Relaxed) && text::ends_with(image, text::MSIEXEC);
        if from_traced || installer_service {
            state::add_pid(pid);
            let flags = if from_traced { 0 } else { proto::FLAG_FOLLOWED };
            state::push(proto::EVENT_PROCESS_START, flags, pid, image, &[]);
        }
    }
}
