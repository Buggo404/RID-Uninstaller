//! Resolves `.lnk` shortcuts.

use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::ApplicationInstallationAndServicing::MsiGetShortcutTargetW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink};
use windows::core::{HSTRING, Interface, PWSTR};

/// COM stays initialised on this thread for as long as the value lives.
pub struct Com(bool);

impl Com {
    pub fn init() -> Com {
        // SAFETY: paired with CoUninitialize in Drop, only when this call succeeded.
        Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() }
        }
    }
}

/// The file a shortcut points to. Requires COM on the calling thread (see [`Com`]).
pub fn target(lnk: &Path) -> Option<PathBuf> {
    // SAFETY: plain COM calls on objects owned by this function; `buf` outlives GetPath.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut::<WIN32_FIND_DATAW>(), SLGP_RAWPATH.0 as u32).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }
}

/// The product code (`{GUID}`) of the Windows Installer product an "advertised" shortcut
/// belongs to. Such shortcuts carry no file path; Installer resolves them when clicked.
pub fn msi_product(lnk: &Path) -> Option<String> {
    let mut code = [0u16; 39];
    // SAFETY: `code` has the 39 characters (GUID + NUL) the API documents for a product code.
    let status = unsafe { MsiGetShortcutTargetW(&HSTRING::from(lnk.as_os_str()), Some(PWSTR(code.as_mut_ptr())), None, None) };
    (status == 0).then(|| String::from_utf16_lossy(&code[..38]))
}
