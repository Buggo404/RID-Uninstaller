//! Generates the kernel bindings from the WDK headers and sets up linking as a kernel driver.
//!
//! Needs the Windows Driver Kit and LLVM (libclang). Override the defaults with
//! `RID_WDK_ROOT`, `RID_WDK_VERSION` and `LIBCLANG_PATH`.

use std::env;
use std::path::PathBuf;

fn main() {
    let kits = PathBuf::from(env::var("RID_WDK_ROOT").unwrap_or_else(|_| r"C:\Program Files (x86)\Windows Kits\10".into()));
    let version = env::var("RID_WDK_VERSION").ok().unwrap_or_else(|| {
        // The newest kit that actually has the kernel-mode headers (SDK-only versions do not).
        let mut versions: Vec<String> = std::fs::read_dir(kits.join("Include"))
            .expect("Windows Kits Include folder not found; set RID_WDK_ROOT")
            .flatten()
            .filter(|e| e.path().join("km").join("fltKernel.h").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        versions.sort();
        versions.pop().expect("no Windows Driver Kit with fltKernel.h found; install the WDK")
    });
    if env::var_os("LIBCLANG_PATH").is_none() && PathBuf::from(r"C:\Program Files\LLVM\bin\libclang.dll").is_file() {
        // SAFETY: the build script is single-threaded at this point.
        unsafe { env::set_var("LIBCLANG_PATH", r"C:\Program Files\LLVM\bin") };
    }

    let include = kits.join("Include").join(&version);
    // A WDK installed without the SDK of the same version lacks the SDK half of `shared`
    // (specstrings.h and friends); the newest installed SDK supplies those.
    let sdk_shared = std::fs::read_dir(kits.join("Include"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("shared"))
        .filter(|shared| shared.join("specstrings.h").is_file())
        .max()
        .expect("no Windows SDK with shared\\specstrings.h found; install the Windows SDK");
    let bindings = bindgen::Builder::default()
        .header("src/wrapper.h")
        .use_core()
        .layout_tests(false)
        .derive_default(true)
        .prepend_enum_name(false)
        .clang_args(["--target=x86_64-pc-windows-msvc", "-fms-compatibility", "-fms-extensions", "-Wno-everything"])
        .clang_args(["-D_AMD64_", "-DAMD64", "-D_WIN64", "-D_KERNEL_MODE", "-DDEPRECATE_DDK_FUNCTIONS=1", "-DNDEBUG"])
        .clang_args(["-D_WIN32_WINNT=0x0A00", "-DWINVER=0x0A00", "-DNTDDI_VERSION=0x0A000007"])
        .clang_args(["km", r"km\crt", "shared"].map(|dir| format!("-I{}", include.join(dir).display())))
        .clang_arg(format!("-I{}", sdk_shared.display()))
        .allowlist_function(
            "Flt(RegisterFilter|StartFiltering|UnregisterFilter|GetFileNameInformation|ReleaseFileNameInformation\
             |ParseFileNameInformation|GetDestinationFileNameInformation|GetRequestorProcessId|IsDirectory)",
        )
        .allowlist_function("Ke(InitializeGuardedMutex|AcquireGuardedMutex|ReleaseGuardedMutex)")
        .allowlist_function("Cm(RegisterCallbackEx|UnRegisterCallback|CallbackGetKeyObjectIDEx|CallbackReleaseKeyObjectIDEx)")
        .allowlist_function("PsSetCreateProcessNotifyRoutineEx|PsGetCurrentProcessId")
        .allowlist_function("WdmlibIoCreateDeviceSecure|Io(DeleteDevice|CreateSymbolicLink|DeleteSymbolicLink)|IofCompleteRequest")
        .allowlist_function("Ke(AcquireSpinLockRaiseToDpc|ReleaseSpinLock|BugCheckEx)|DbgPrint")
        .allowlist_type("FLT_REGISTRATION|FLT_OPERATION_REGISTRATION|FLT_PREOP_CALLBACK_STATUS|FLT_POSTOP_CALLBACK_STATUS")
        .allowlist_type("FLT_CALLBACK_DATA|FLT_RELATED_OBJECTS|KGUARDED_MUTEX")
        .allowlist_type("FLT_FILE_NAME_INFORMATION|FILE_RENAME_INFORMATION|FILE_DISPOSITION_INFORMATION(_EX)?|FILE_INFORMATION_CLASS")
        .allowlist_type("REG_NOTIFY_CLASS|REG_(POST_OPERATION|CREATE_KEY|SET_VALUE_KEY|DELETE_KEY|DELETE_VALUE_KEY)_INFORMATION(_V1)?")
        .allowlist_type("PS_CREATE_NOTIFY_INFO|DRIVER_OBJECT|DEVICE_OBJECT|IRP|IO_STACK_LOCATION|UNICODE_STRING")
        .allowlist_var("IRP_MJ_.*|FLT_REGISTRATION_VERSION|FLT_FILE_NAME_.*|FLTFL_.*|FILE_DEVICE_.*|FILE_(CREATED|DIRECTORY_FILE|DELETE_ON_CLOSE)")
        .allowlist_var("REG_CREATED_NEW_KEY|IO_NO_INCREMENT|FILE_DISPOSITION_DELETE")
        .generate()
        .expect("failed to generate kernel bindings");
    let out = PathBuf::from(env::var("OUT_DIR").expect("set by cargo"));
    bindings.write_to_file(out.join("bindings.rs")).expect("failed to write bindings");

    println!("cargo:rustc-link-search=native={}", kits.join("Lib").join(&version).join("km").join("x64").display());
    for lib in ["ntoskrnl", "hal", "fltMgr", "wdmsec", "BufferOverflowFastFailK"] {
        println!("cargo:rustc-link-lib={lib}");
    }
    for arg in [
        "/NODEFAULTLIB",
        "/SUBSYSTEM:NATIVE",
        "/DRIVER",
        "/ENTRY:DriverEntry",
        "/MANIFEST:NO",
        "/NXCOMPAT",
        "/DYNAMICBASE",
        // PsSetCreateProcessNotifyRoutineEx refuses images that are not marked for forced integrity checking.
        "/INTEGRITYCHECK",
        "/OPT:REF,ICF",
    ] {
        println!("cargo:rustc-cdylib-link-arg={arg}");
    }
    println!("cargo:rerun-if-changed=src/wrapper.h");
    println!("cargo:rerun-if-env-changed=RID_WDK_ROOT");
    println!("cargo:rerun-if-env-changed=RID_WDK_VERSION");
}
