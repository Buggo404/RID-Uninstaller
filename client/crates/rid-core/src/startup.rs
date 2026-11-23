//! Programs that start with Windows: registry Run keys, Startup folders, logon tasks and
//! Store app startup tasks. Disabling uses the same switches as Task Manager.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ, KEY_WOW64_64KEY, RegType};
use winreg::{RegKey, RegValue};

use crate::safety::{Roots, norm};
use crate::scan::Target;
use crate::{cmdline, links, ops, procs, tasks};

const CV: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion";
const CV32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion";
const APPROVED: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
const STORE_TASKS: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    HklmRun,
    HklmRun32,
    HklmRunOnce,
    HklmRunOnce32,
    HkcuRun,
    HkcuRunOnce,
    FolderUser,
    FolderCommon,
    Task,
    StoreApp,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    NotRunning,
    Disabled,
    /// The program it points to no longer exists.
    Invalid,
}

#[derive(Serialize, Clone, Debug)]
pub struct Item {
    pub source: Source,
    /// Value name, file name, task path, or `<package family>\<task id>`: unique within a source.
    pub name: String,
    pub title: String,
    pub command: String,
    pub exe: String,
    pub company: String,
    pub status: Status,
    /// RunOnce entries have no on/off switch in Windows.
    pub can_disable: bool,
    pub is_32bit: bool,
}

impl Source {
    /// Hive, key holding the entries, and the StartupApproved subkey that switches them.
    fn registry(self) -> Option<(&'static str, String, Option<&'static str>)> {
        Some(match self {
            Source::HklmRun => ("HKLM", format!(r"{CV}\Run"), Some("Run")),
            Source::HklmRun32 => ("HKLM", format!(r"{CV32}\Run"), Some("Run32")),
            Source::HklmRunOnce => ("HKLM", format!(r"{CV}\RunOnce"), None),
            Source::HklmRunOnce32 => ("HKLM", format!(r"{CV32}\RunOnce"), None),
            Source::HkcuRun => ("HKCU", format!(r"{CV}\Run"), Some("Run")),
            Source::HkcuRunOnce => ("HKCU", format!(r"{CV}\RunOnce"), None),
            _ => return None,
        })
    }

    fn folder(self, roots: &Roots) -> Option<(&'static str, PathBuf)> {
        // start_menus is [per-user, all-users].
        match self {
            Source::FolderUser => Some(("HKCU", roots.start_menus.first()?.join("Startup"))),
            Source::FolderCommon => Some(("HKLM", roots.start_menus.get(1)?.join("Startup"))),
            _ => None,
        }
    }
}

fn hive(name: &str) -> RegKey {
    RegKey::predef(if name == "HKLM" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
}

/// Task Manager's switch: a binary value whose first byte is even when enabled, odd when disabled.
fn approved(hive_name: &str, list: &str, name: &str) -> bool {
    hive(hive_name)
        .open_subkey_with_flags(format!(r"{APPROVED}\{list}"), KEY_READ | KEY_WOW64_64KEY)
        .and_then(|k| k.get_raw_value(name))
        .map(|v| v.bytes.first().is_none_or(|b| b % 2 == 0))
        .unwrap_or(true)
}

fn set_approved(hive_name: &str, list: &str, name: &str, enabled: bool) -> io::Result<()> {
    let (key, _) = hive(hive_name).create_subkey_with_flags(format!(r"{APPROVED}\{list}"), KEY_ALL_ACCESS | KEY_WOW64_64KEY)?;
    let mut bytes = vec![if enabled { 2u8 } else { 3 }, 0, 0, 0];
    // Windows stores the time of disabling after the flag; all zeroes when enabled.
    let ticks = if enabled {
        0u64
    } else {
        let since_1970 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() / 100).unwrap_or(0);
        since_1970 as u64 + 116_444_736_000_000_000
    };
    bytes.extend(ticks.to_le_bytes());
    key.set_raw_value(name, &RegValue { vtype: RegType::REG_BINARY, bytes: bytes.into() })
}

/// The `CompanyName` from an executable's version resource.
fn company(exe: &str) -> String {
    version_string(exe, "CompanyName")
}

/// One named string (`CompanyName`, `FileDescription`, ...) from an executable's version resource.
pub(crate) fn version_string(exe: &str, field: &str) -> String {
    let path = HSTRING::from(exe);
    // SAFETY: the buffer has the size the API asked for; returned pointers point into it and
    // are only read while it is alive.
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return String::new();
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()).is_err() {
            return String::new();
        }
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let query = |sub: &str, ptr: &mut *mut core::ffi::c_void, len: &mut u32| {
            VerQueryValueW(data.as_ptr().cast(), &HSTRING::from(sub), ptr, len).as_bool() && !ptr.is_null() && *len > 0
        };
        if !query(r"\VarFileInfo\Translation", &mut ptr, &mut len) || len < 4 {
            return String::new();
        }
        let lang = *(ptr as *const [u16; 2]);
        let sub = format!(r"\StringFileInfo\{:04x}{:04x}\{field}", lang[0], lang[1]);
        if !query(&sub, &mut ptr, &mut len) {
            return String::new();
        }
        let text = std::slice::from_raw_parts(ptr as *const u16, len as usize);
        String::from_utf16_lossy(text).trim_end_matches('\0').trim().to_string()
    }
}

struct Lister {
    running: Vec<String>,
    items: Vec<Item>,
}

impl Lister {
    fn push(&mut self, source: Source, name: String, title: String, command: String, exe: String, enabled: bool, is_32bit: bool) {
        let expanded = expand_env(&exe);
        let status = if !enabled {
            Status::Disabled
        } else if expanded.is_empty() || !Path::new(&expanded).is_file() {
            Status::Invalid
        } else if norm(Path::new(&expanded)).is_some_and(|p| self.running.contains(&p)) {
            Status::Running
        } else {
            Status::NotRunning
        };
        let can_disable = !matches!(source, Source::HklmRunOnce | Source::HklmRunOnce32 | Source::HkcuRunOnce);
        self.items.push(Item { source, name, title, command, company: company(&expanded), exe: expanded, status, can_disable, is_32bit });
    }
}

/// Expands `%SystemRoot%`-style references; Run values are often REG_EXPAND_SZ.
pub(crate) fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out + rest
}

pub fn list(roots: &Roots) -> Vec<Item> {
    let running = procs::all_images().iter().filter_map(|p| norm(Path::new(p))).collect();
    let mut l = Lister { running, items: Vec::new() };

    for source in [Source::HklmRun, Source::HklmRun32, Source::HklmRunOnce, Source::HklmRunOnce32, Source::HkcuRun, Source::HkcuRunOnce] {
        let Some((hive_name, key, switch)) = source.registry() else { continue };
        let Ok(k) = hive(hive_name).open_subkey_with_flags(&key, KEY_READ | KEY_WOW64_64KEY) else { continue };
        for (name, value) in k.enum_values().flatten() {
            if name.is_empty() {
                continue;
            }
            let command = value.to_string();
            let exe = cmdline::exe_of(&expand_env(&command)).unwrap_or_default().to_string();
            let enabled = switch.is_none_or(|list| approved(hive_name, list, &name));
            let is_32bit = matches!(source, Source::HklmRun32 | Source::HklmRunOnce32);
            l.push(source, name.clone(), name, command, exe, enabled, is_32bit);
        }
    }

    let _com = links::Com::init();
    for source in [Source::FolderUser, Source::FolderCommon] {
        let Some((hive_name, dir)) = source.folder(roots) else { continue };
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if file.eq_ignore_ascii_case("desktop.ini") || !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let is_link = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"));
            let exe = if is_link { links::target(&path) } else { Some(path.clone()) }
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let enabled = approved(hive_name, "StartupFolder", &file);
            l.push(source, file, title, path.to_string_lossy().into_owned(), exe, enabled, false);
        }
    }

    for task in tasks::logon_tasks().unwrap_or_default() {
        let command = format!("{} {}", task.exe, task.arguments).trim().to_string();
        let exe = task.exe.trim_matches('"').to_string();
        l.push(Source::Task, task.path, task.name, command, exe, task.enabled, false);
    }

    if let Ok(root) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(STORE_TASKS) {
        for family in root.enum_keys().flatten() {
            let Ok(package) = root.open_subkey(&family) else { continue };
            for task in package.enum_keys().flatten() {
                let Some(state) = package.open_subkey(&task).ok().and_then(|k| k.get_value::<u32, _>("State").ok()) else { continue };
                // 2 = enabled, 4 = enabled by policy; 0, 1 and 3 are the disabled states.
                let enabled = matches!(state, 2 | 4);
                let title = family.split('_').next().unwrap_or(&family).to_string();
                l.items.push(Item {
                    source: Source::StoreApp,
                    name: format!(r"{family}\{task}"),
                    title,
                    command: String::new(),
                    exe: String::new(),
                    company: String::new(),
                    status: if enabled { Status::NotRunning } else { Status::Disabled },
                    can_disable: state != 4,
                    is_32bit: false,
                });
            }
        }
    }

    l.items.sort_by_cached_key(|i| i.title.to_lowercase());
    l.items
}

pub fn set_enabled(source: Source, name: &str, enabled: bool, roots: &Roots) -> io::Result<()> {
    if let Some((hive_name, _, switch)) = source.registry() {
        let list = switch.ok_or_else(|| io::Error::other("RunOnce entries cannot be disabled, only deleted"))?;
        return set_approved(hive_name, list, name, enabled);
    }
    if let Some((hive_name, _)) = source.folder(roots) {
        return set_approved(hive_name, "StartupFolder", name, enabled);
    }
    match source {
        Source::Task => tasks::set_enabled(name, enabled),
        _ => {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(format!(r"{STORE_TASKS}\{name}"))?;
            key.set_value("State", &if enabled { 2u32 } else { 1u32 })
        }
    }
}

/// Deletes a startup entry. Registry values and Startup-folder files go to quarantine and can
/// be restored; scheduled tasks are deleted for good. Store app tasks cannot be deleted.
pub fn delete(source: Source, name: &str, roots: &Roots) -> io::Result<()> {
    let target = if let Some((hive_name, key, _)) = source.registry() {
        Target::RegValue { key: format!(r"{hive_name}\{key}"), name: name.into() }
    } else if let Some((_, dir)) = source.folder(roots) {
        if name.contains(['\\', '/']) {
            return Err(io::Error::other("not a file name"));
        }
        Target::File { path: dir.join(name).to_string_lossy().into_owned() }
    } else if source == Source::Task {
        return tasks::delete(name);
    } else {
        return Err(io::Error::other("a Store app's startup task can only be disabled"));
    };
    let report = ops::remove(&format!("Startup: {name}"), &[target], roots, &[], &[])?;
    match report.log.first() {
        Some(entry) if !entry.ok => Err(io::Error::other(entry.message.clone())),
        _ => Ok(()),
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AddTo {
    HkcuRun,
    HklmRun,
    FolderUser,
    FolderCommon,
    Task,
    /// A logon task with the highest privileges: starts elevated without a UAC prompt.
    TaskElevated,
}

pub fn add(to: AddTo, name: &str, exe: &str, arguments: &str, roots: &Roots) -> io::Result<()> {
    let name = name.trim();
    if name.is_empty() || name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(io::Error::other("the name is empty or contains characters not allowed in a file name"));
    }
    if !Path::new(exe).is_file() {
        return Err(io::Error::other(format!("no such file: {exe}")));
    }
    let command = if arguments.trim().is_empty() { format!("\"{exe}\"") } else { format!("\"{exe}\" {}", arguments.trim()) };
    match to {
        AddTo::HkcuRun | AddTo::HklmRun => {
            let hive_name = if to == AddTo::HklmRun { "HKLM" } else { "HKCU" };
            let (key, _) = hive(hive_name).create_subkey_with_flags(format!(r"{CV}\Run"), KEY_ALL_ACCESS | KEY_WOW64_64KEY)?;
            if key.get_raw_value(name).is_ok() {
                return Err(io::Error::other(format!("a startup entry named '{name}' already exists")));
            }
            key.set_value(name, &command)
        }
        AddTo::FolderUser | AddTo::FolderCommon => {
            let source = if to == AddTo::FolderCommon { Source::FolderCommon } else { Source::FolderUser };
            let (_, dir) = source.folder(roots).ok_or_else(|| io::Error::other("no Startup folder"))?;
            let link_path = dir.join(format!("{name}.lnk"));
            if link_path.exists() {
                return Err(io::Error::other(format!("a startup entry named '{name}' already exists")));
            }
            fs::create_dir_all(&dir)?;
            let _com = links::Com::init();
            // SAFETY: plain COM calls on an object owned by this block.
            unsafe {
                (|| {
                    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                    link.SetPath(&HSTRING::from(exe))?;
                    link.SetArguments(&HSTRING::from(arguments.trim()))?;
                    if let Some(dir) = Path::new(exe).parent() {
                        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
                    }
                    link.cast::<IPersistFile>()?.Save(&HSTRING::from(link_path.as_os_str()), true)
                })()
            }
            .map_err(|e: windows::core::Error| io::Error::other(e.message()))
        }
        AddTo::Task | AddTo::TaskElevated => tasks::create(name, exe, arguments.trim(), to == AddTo::TaskElevated),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_references_are_expanded() {
        // SAFETY: test-only, set before any thread reads it.
        unsafe { std::env::set_var("RID_TEST_DIR", r"C:\Apps") };
        assert_eq!(expand_env(r"%RID_TEST_DIR%\a.exe /x"), r"C:\Apps\a.exe /x");
        assert_eq!(expand_env(r#""%RID_TEST_DIR%\a b.exe" 50%"#), r#""C:\Apps\a b.exe" 50%"#);
        assert_eq!(expand_env("%RID_NO_SUCH_VAR%\\x"), "%RID_NO_SUCH_VAR%\\x");
        assert_eq!(expand_env("100%"), "100%");
    }

    /// Uses a value of its own under HKCU and removes it afterwards.
    #[test]
    fn disabling_writes_the_switch_task_manager_reads() {
        let name = format!("RidStartupTest{}", std::process::id());
        assert!(approved("HKCU", "Run", &name), "no switch means enabled");
        set_approved("HKCU", "Run", &name, false).unwrap();
        let disabled = approved("HKCU", "Run", &name);
        let key = hive("HKCU").open_subkey_with_flags(format!(r"{APPROVED}\Run"), KEY_ALL_ACCESS).unwrap();
        let raw = key.get_raw_value(&name).map(|v| v.bytes.to_vec());
        set_approved("HKCU", "Run", &name, true).unwrap();
        let enabled = approved("HKCU", "Run", &name);
        key.delete_value(&name).unwrap();

        assert!(!disabled && enabled);
        let raw = raw.unwrap();
        assert_eq!((raw.len(), raw[0]), (12, 3));
        assert!(raw[4..].iter().any(|b| *b != 0), "the time of disabling is recorded");
    }

    #[test]
    fn company_name_comes_from_the_version_resource() {
        assert_eq!(company(&ops::system32("cmd.exe").to_string_lossy()), "Microsoft Corporation");
        assert_eq!(company(r"C:\no\such.exe"), "");
    }

    #[test]
    fn listing_reads_this_machine_without_failing() {
        let items = list(&Roots::from_system());
        for i in &items {
            assert!(!i.name.is_empty() && !i.title.is_empty(), "{i:?}");
        }
    }

    #[test]
    fn additions_are_validated() {
        let roots = Roots::from_system();
        let cmd = ops::system32("cmd.exe").to_string_lossy().into_owned();
        assert!(add(AddTo::HkcuRun, "", &cmd, "", &roots).is_err());
        assert!(add(AddTo::HkcuRun, r"a\b", &cmd, "", &roots).is_err());
        assert!(add(AddTo::HkcuRun, "RidTest", r"C:\no\such.exe", "", &roots).is_err());
        assert!(delete(Source::FolderUser, r"..\..\x.lnk", &roots).is_err());
        assert!(delete(Source::StoreApp, "x", &roots).is_err());
        assert!(set_enabled(Source::HkcuRunOnce, "x", false, &roots).is_err());
    }
}
