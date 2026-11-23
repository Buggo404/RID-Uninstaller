//! What a traced installation left on the machine, and the per-program records of it.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

use crate::paths;
use crate::safety::{self, norm, within};
use crate::scan::Target;
use crate::trace::{Change, Event};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Record {
    /// The installed program this recording belongs to (an inventory id), or `TRACE:<name>`
    /// for an installation that registered no uninstall entry.
    pub app_id: String,
    pub name: String,
    pub created: u64,
    pub items: Vec<Target>,
    /// Other programs the same installation registered; removed together on request.
    #[serde(default)]
    pub dependents: Vec<String>,
}

fn key_exists(key: &str) -> bool {
    let Some((hive, path)) = key.split_once('\\') else { return false };
    let root = if hive == "HKLM" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
    RegKey::predef(root).open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY).is_ok()
}

fn value_exists(key: &str, name: &str) -> bool {
    let Some((hive, path)) = key.split_once('\\') else { return false };
    let root = if hive == "HKLM" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
    RegKey::predef(root)
        .open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
        .is_ok_and(|k| k.get_raw_value(name).is_ok())
}

fn key_within(inner: &str, outer: &str) -> bool {
    let (inner, outer) = (inner.to_lowercase(), outer.to_lowercase());
    inner == outer || inner.strip_prefix(&outer).is_some_and(|rest| rest.starts_with('\\'))
}

/// The net result of a recording: what was created and is still there, outermost items only.
/// `exists` hooks let tests run without touching the machine.
fn reduce_with(events: &[Event], path_exists: impl Fn(&str) -> bool, key_there: impl Fn(&str) -> bool, value_there: impl Fn(&str, &str) -> bool) -> Vec<Target> {
    // Keyed by normalised path / lower-cased key so that case differences collapse.
    let mut paths: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    let mut values: BTreeMap<(String, String), (String, String)> = BTreeMap::new();

    for event in events {
        match &event.change {
            Change::Created { path, dir } => {
                if let Some(n) = norm(Path::new(path)) {
                    paths.insert(n, (path.clone(), *dir));
                }
            }
            // An installer typically writes a temporary file and renames it into place.
            Change::Renamed { from, to } => {
                let (Some(from_n), Some(to_n)) = (norm(Path::new(from)), norm(Path::new(to))) else { continue };
                let moved: Vec<String> = paths.keys().filter(|p| within(p, &from_n)).cloned().collect();
                for old in moved {
                    let Some((original, dir)) = paths.remove(&old) else { continue };
                    let tail = &original[from.len().min(original.len())..];
                    paths.insert(format!("{to_n}{}", &old[from_n.len()..]), (format!("{to}{tail}"), dir));
                }
            }
            Change::Deleted { path } => {
                if let Some(n) = norm(Path::new(path)) {
                    paths.retain(|p, _| !within(p, &n));
                }
            }
            Change::KeyCreated { key } => {
                keys.insert(key.to_lowercase(), key.clone());
            }
            Change::KeyDeleted { key } => {
                keys.retain(|_, k| !key_within(k, key));
                values.retain(|_, (k, _)| !key_within(k, key));
            }
            Change::ValueSet { key, name } => {
                values.insert((key.to_lowercase(), name.to_lowercase()), (key.clone(), name.clone()));
            }
            Change::ValueDeleted { key, name } => {
                values.remove(&(key.to_lowercase(), name.to_lowercase()));
            }
            Change::ProcessStarted { .. } | Change::ProcessExited => {}
        }
    }

    let mut out = Vec::new();
    let created_dirs: Vec<&String> = paths.iter().filter(|(_, (_, dir))| *dir).map(|(n, _)| n).collect();
    for (n, (path, dir)) in &paths {
        // Whatever lies inside a folder the installation created goes with that folder.
        let nested = created_dirs.iter().any(|d| *d != n && within(n, d));
        if !nested && path_exists(path) {
            out.push(if *dir { Target::Dir { path: path.clone() } } else { Target::File { path: path.clone() } });
        }
    }
    for key in keys.values() {
        let nested = keys.values().any(|other| other != key && key_within(key, other));
        if !nested && key_there(key) {
            out.push(Target::RegKey { key: key.clone() });
        }
    }
    for (key, name) in values.values() {
        // A value written into a key that already existed may have replaced an earlier one,
        // and the tracer does not know. Only autostart entries, which Rid may remove anyway, are kept.
        let in_created_key = keys.values().any(|k| key_within(key, k));
        if !in_created_key && safety::check_value(key).is_ok() && value_there(key, name) {
            out.push(Target::RegValue { key: key.clone(), name: name.clone() });
        }
    }
    out
}

pub fn reduce(events: &[Event]) -> Vec<Target> {
    reduce_with(events, |p| fs::symlink_metadata(p).is_ok(), key_exists, value_exists)
}

fn dir() -> io::Result<PathBuf> {
    let dir = paths::secure_dir()?.join("traces");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Ids contain characters that are not valid in file names; the hex form is always safe.
fn file_of(app_id: &str) -> io::Result<PathBuf> {
    let hex: String = app_id.bytes().map(|b| format!("{b:02x}")).collect();
    Ok(dir()?.join(format!("{hex}.json")))
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn save(record: &Record) -> io::Result<()> {
    fs::write(file_of(&record.app_id)?, serde_json::to_vec_pretty(record)?)
}

pub fn load(app_id: &str) -> Option<Record> {
    serde_json::from_slice(&fs::read(file_of(app_id).ok()?).ok()?).ok()
}

pub fn list() -> Vec<Record> {
    dir()
        .and_then(fs::read_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
        .collect()
}

pub fn forget(app_id: &str) -> io::Result<()> {
    match fs::remove_file(file_of(app_id)?) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Adds a later recording (first run, an update) to an existing record.
pub fn append(app_id: &str, items: Vec<Target>) -> io::Result<()> {
    let mut record = load(app_id).ok_or_else(|| io::Error::other("this program has no traced data"))?;
    for item in items {
        if !record.items.contains(&item) {
            record.items.push(item);
        }
    }
    save(&record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(change: Change) -> Event {
        Event { pid: 1, change }
    }
    fn created(path: &str, dir: bool) -> Event {
        event(Change::Created { path: path.into(), dir })
    }
    fn key(k: &str) -> Event {
        event(Change::KeyCreated { key: k.into() })
    }
    fn all(events: &[Event]) -> Vec<Target> {
        reduce_with(events, |_| true, |_| true, |_, _| true)
    }
    fn dir(p: &str) -> Target {
        Target::Dir { path: p.into() }
    }
    fn file(p: &str) -> Target {
        Target::File { path: p.into() }
    }

    #[test]
    fn contents_of_a_created_folder_collapse_into_it() {
        let got = all(&[
            created(r"C:\Program Files\App", true),
            created(r"C:\Program Files\App\app.exe", false),
            created(r"C:\Program Files\App\Lang", true),
            created(r"C:\Program Files\App\Lang\en.txt", false),
            created(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\App.lnk", false),
        ]);
        assert_eq!(got, [dir(r"C:\Program Files\App"), file(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\App.lnk")]);
    }

    #[test]
    fn temporary_files_and_renames_are_followed() {
        let got = all(&[
            created(r"C:\Users\me\AppData\Local\Temp\setup.tmp", false),
            event(Change::Deleted { path: r"C:\Users\me\AppData\Local\Temp\setup.tmp".into() }),
            created(r"C:\Windows\System32\drv.sys.new", false),
            event(Change::Renamed { from: r"C:\Windows\System32\drv.sys.new".into(), to: r"C:\Windows\System32\drv.sys".into() }),
            created(r"C:\Stage", true),
            created(r"C:\Stage\a.dll", false),
            event(Change::Renamed { from: r"C:\Stage".into(), to: r"C:\Apps\Final".into() }),
            // A file that existed before and is merely renamed is not the installation's.
            event(Change::Renamed { from: r"C:\Existing\old.ini".into(), to: r"C:\Existing\old.bak".into() }),
        ]);
        assert_eq!(got, [dir(r"C:\Apps\Final"), file(r"C:\Windows\System32\drv.sys")]);
    }

    #[test]
    fn only_what_still_exists_is_kept() {
        let events = [created(r"C:\Apps\Gone", true), created(r"C:\Apps\Here", true), key(r"HKLM\SOFTWARE\Gone"), key(r"HKLM\SOFTWARE\Here")];
        let got = reduce_with(&events, |p| p.ends_with("Here"), |k| k.ends_with("Here"), |_, _| true);
        assert_eq!(got, [dir(r"C:\Apps\Here"), Target::RegKey { key: r"HKLM\SOFTWARE\Here".into() }]);
    }

    #[test]
    fn registry_keys_and_values() {
        let run = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
        let got = all(&[
            key(r"HKLM\SOFTWARE\Vendor"),
            key(r"HKLM\SOFTWARE\Vendor\App"),
            event(Change::ValueSet { key: r"HKLM\SOFTWARE\Vendor\App".into(), name: "Path".into() }),
            key(r"HKLM\SOFTWARE\Classes\CLSID\{A}"),
            key(r"HKLM\SOFTWARE\Temp"),
            event(Change::KeyDeleted { key: r"HKLM\SOFTWARE\Temp".into() }),
            event(Change::ValueSet { key: run.into(), name: "App".into() }),
            // Written into a key that existed before: may have replaced something, so it is left alone.
            event(Change::ValueSet { key: r"HKLM\SOFTWARE\Classes\.txt".into(), name: "".into() }),
            event(Change::ValueSet { key: run.into(), name: "Once".into() }),
            event(Change::ValueDeleted { key: run.into(), name: "once".into() }),
        ]);
        assert_eq!(
            got,
            [
                Target::RegKey { key: r"HKLM\SOFTWARE\Classes\CLSID\{A}".into() },
                Target::RegKey { key: r"HKLM\SOFTWARE\Vendor".into() },
                Target::RegValue { key: run.into(), name: "App".into() },
            ]
        );
    }
}
