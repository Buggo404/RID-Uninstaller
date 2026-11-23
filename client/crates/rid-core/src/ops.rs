//! Removal with undo. Nothing is destroyed here: files are moved into a quarantine folder on
//! their own volume, registry data is exported first, and `restore` puts a batch back.

use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
use windows::core::{HSTRING, PCWSTR};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ, KEY_WOW64_64KEY};
use winreg::{RegKey, RegValue};

use crate::safety::{self, Roots, norm, within};
use crate::scan::Target;
use crate::paths;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const QUARANTINE: &str = "RidQuarantine";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Stored {
    Path { original: String, stored: String },
    /// `file` is a `reg export` of the key, kept next to the manifest.
    RegKey { key: String, file: String },
    RegValue { key: String, name: String, vtype: u32, data: Vec<u8> },
    Service { name: String, file: String },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Manifest {
    label: String,
    created: u64,
    items: Vec<Stored>,
}

#[derive(Serialize, Clone, Debug)]
pub struct LogEntry {
    pub target: Target,
    pub ok: bool,
    pub message: String,
}

#[derive(Serialize, Debug)]
pub struct Report {
    /// `None` when nothing was removed, so there is nothing to undo.
    pub batch: Option<String>,
    pub log: Vec<LogEntry>,
    /// Files in use. Windows moves them into quarantine at the next restart (and removes the
    /// folders they leave empty), so they can be restored after that restart.
    pub pending_reboot: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct BatchInfo {
    pub id: String,
    pub label: String,
    pub created: u64,
    pub items: usize,
    pub size_kb: u64,
}

pub(crate) fn system32(exe: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join(exe)
}

fn run(exe: &str, args: &[&str]) -> io::Result<()> {
    let out = Command::new(system32(exe)).args(args).creation_flags(CREATE_NO_WINDOW).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{exe} failed: {}", String::from_utf8_lossy(&out.stderr).trim())))
    }
}

fn batches_dir() -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(base) = tests::BASE.with(|b| b.borrow().clone()) {
        return Ok(base.join("manifests"));
    }
    Ok(paths::secure_dir()?.join("quarantine"))
}

/// Where removed files of the volume `drive` (`C:`) wait. On the same volume as the
/// originals, so quarantining is a rename: instant, and no second copy on disk.
fn payload_base(drive: &str) -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(base) = tests::BASE.with(|b| b.borrow().clone()) {
        fs::create_dir_all(base.join("payload"))?;
        return Ok(base.join("payload"));
    }
    let base = PathBuf::from(format!(r"{drive}\{QUARANTINE}"));
    fs::create_dir_all(&base)?;
    // Drive roots let ordinary users write. Only the folder itself is locked: the files that
    // wait inside are the user's and keep their own owner and permissions for the way back.
    paths::lock_down(&base, false)?;
    Ok(base)
}

fn reg_type(id: u32) -> winreg::enums::RegType {
    use winreg::enums::RegType::*;
    match id {
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        7 => REG_MULTI_SZ,
        11 => REG_QWORD,
        _ => REG_SZ,
    }
}

/// Batch ids are generated here; anything else (`..`, separators) must never reach the filesystem.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

fn split_key(key: &str) -> io::Result<(RegKey, &str)> {
    let (hive, path) = key.split_once('\\').ok_or_else(|| io::Error::other("not a registry key"))?;
    let root = match hive.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        _ => return Err(io::Error::other("not a registry key under HKLM or HKCU")),
    };
    Ok((RegKey::predef(root), path))
}

/// `reg.exe` wants the long hive name spelled the way it prints it.
fn reg_exe_name(key: &str) -> String {
    match key.split_once('\\') {
        Some((h, rest)) if h.eq_ignore_ascii_case("HKEY_LOCAL_MACHINE") => format!("HKLM\\{rest}"),
        Some((h, rest)) if h.eq_ignore_ascii_case("HKEY_CURRENT_USER") => format!("HKCU\\{rest}"),
        _ => key.to_string(),
    }
}

fn delete_on_reboot(path: &Path) -> bool {
    // SAFETY: the HSTRING is a valid NUL-terminated string for the duration of the call.
    unsafe { MoveFileExW(&HSTRING::from(path.as_os_str()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT) }.is_ok()
}

/// Moves `from` to `to`. When a folder cannot be moved whole because something inside is in
/// use, everything movable is moved and the rest is queued for deletion at restart.
fn move_tree(from: &Path, to: &Path, pending: &mut Vec<String>) -> io::Result<()> {
    let whole = fs::rename(from, to);
    let is_real_dir = fs::symlink_metadata(from).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    let Err(error) = whole else { return Ok(()) };
    if !is_real_dir {
        // In use: Windows carries out the same move at the next restart, so the file ends
        // up in quarantine like everything else and stays restorable.
        // SAFETY: both HSTRINGs are valid NUL-terminated strings for the duration of the call.
        let queued = unsafe { MoveFileExW(&HSTRING::from(from.as_os_str()), &HSTRING::from(to.as_os_str()), MOVEFILE_DELAY_UNTIL_REBOOT) };
        return if queued.is_ok() {
            pending.push(from.to_string_lossy().into_owned());
            Ok(())
        } else {
            Err(error)
        };
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        move_tree(&entry.path(), &to.join(entry.file_name()), pending)?;
    }
    // Empty now unless files are waiting for the restart; then the folder waits with them.
    if fs::remove_dir(from).is_err() && delete_on_reboot(from) {
        pending.push(from.to_string_lossy().into_owned());
    }
    Ok(())
}

/// Removes folders left empty by a removal (an empty vendor folder, an empty Start Menu group).
fn prune_empty_parents(path: &Path, roots: &Roots, others: &[PathBuf]) {
    let mut dir = path.parent();
    while let Some(d) = dir {
        // remove_dir only succeeds on an empty folder; the safety check keeps system folders.
        if safety::check_path(d, roots, others).is_err() || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

struct Removal<'a> {
    id: String,
    dir: PathBuf,
    roots: &'a Roots,
    others: &'a [PathBuf],
    /// Keys an install recording proved the program created; checked with the wider traced rules.
    traced_keys: &'a [String],
    items: Vec<Stored>,
    pending: Vec<String>,
    /// Per-volume quarantine folders this removal created.
    batch_dirs: Vec<PathBuf>,
}

impl Removal<'_> {
    fn path(&mut self, raw: &str) -> io::Result<String> {
        // Checked before resolving: resolving would quietly complete a relative path from the current folder.
        norm(Path::new(raw)).ok_or_else(|| io::Error::other(safety::Denied::Malformed))?;
        let path = safety::resolve(Path::new(raw));
        safety::check_path(&path, self.roots, self.others).map_err(io::Error::other)?;
        if fs::symlink_metadata(&path).is_err() {
            return Ok("already gone".into());
        }
        let text = path.to_string_lossy().into_owned();
        // Same volume as the original, so the move is a rename and needs no extra space.
        let batch = payload_base(&text[..2])?.join(&self.id);
        if !self.batch_dirs.contains(&batch) {
            // Must be new: a folder someone else put there first could be theirs to write to.
            fs::create_dir(&batch)?;
            self.batch_dirs.push(batch.clone());
        }
        let stored = batch
            .join(self.items.len().to_string())
            .join(path.file_name().unwrap_or_default());
        fs::create_dir_all(stored.parent().unwrap_or(Path::new("")))?;
        move_tree(&path, &stored, &mut self.pending)?;
        self.items.push(Stored::Path { original: text, stored: stored.to_string_lossy().into_owned() });
        prune_empty_parents(&path, self.roots, self.others);
        Ok("moved to quarantine".into())
    }

    fn export(&self, key: &str) -> io::Result<String> {
        let file = format!("{}.reg", self.items.len());
        let dest = self.dir.join(&file);
        run("reg.exe", &["export", &reg_exe_name(key), &dest.to_string_lossy(), "/y", "/reg:64"])?;
        Ok(file)
    }

    fn reg_key(&mut self, key: &str) -> io::Result<String> {
        if self.traced_keys.iter().any(|t| t.eq_ignore_ascii_case(key)) {
            safety::check_key_traced(key).map_err(io::Error::other)?;
        } else {
            safety::check_key(key).map_err(io::Error::other)?;
        }
        let (root, path) = split_key(key)?;
        if root.open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY).is_err() {
            return Ok("already gone".into());
        }
        let file = self.export(key)?;
        let (parent, name) = path.rsplit_once('\\').ok_or_else(|| io::Error::other("key has no parent"))?;
        root.open_subkey_with_flags(parent, KEY_ALL_ACCESS | KEY_WOW64_64KEY)?.delete_subkey_all(name)?;
        self.items.push(Stored::RegKey { key: key.into(), file });
        Ok("exported and deleted".into())
    }

    fn reg_value(&mut self, key: &str, name: &str) -> io::Result<String> {
        safety::check_value(key).map_err(io::Error::other)?;
        let (root, path) = split_key(key)?;
        let k = root.open_subkey_with_flags(path, KEY_ALL_ACCESS | KEY_WOW64_64KEY)?;
        let Ok(value) = k.get_raw_value(name) else { return Ok("already gone".into()) };
        k.delete_value(name)?;
        self.items.push(Stored::RegValue {
            key: key.into(),
            name: name.into(),
            vtype: value.vtype.clone() as u32,
            data: value.bytes.to_vec(),
        });
        Ok("saved and deleted".into())
    }

    fn service(&mut self, name: &str) -> io::Result<String> {
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || " ._-".contains(c)) {
            return Err(io::Error::other("not a service name"));
        }
        let key = format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{name}");
        let (root, path) = split_key(&key)?;
        let Ok(k) = root.open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY) else {
            return Ok("already gone".into());
        };
        // Services hosted by Windows itself (svchost, drivers in System32) are never a program's leftovers.
        let image: String = k.get_value("ImagePath").unwrap_or_default();
        let image = image.trim().trim_start_matches('"').trim_start_matches(r"\??\").to_lowercase();
        let windows = norm(&self.roots.windows).unwrap_or_default();
        if !image.as_bytes().get(1).is_some_and(|b| *b == b':') || within(&image, &windows) {
            return Err(io::Error::other(safety::Denied::Protected));
        }
        let file = self.export(&key)?;
        let _ = run("sc.exe", &["stop", name]); // not running is fine
        run("sc.exe", &["delete", name])?;
        self.items.push(Stored::Service { name: name.into(), file });
        Ok("stopped and deleted".into())
    }
}

/// Removes `targets`, each re-checked against the safety rules. `other_programs` are the
/// folders of every installed program that is not being removed. `traced_keys` are registry
/// keys that an install recording of the program being removed shows it created.
pub fn remove(label: &str, targets: &[Target], roots: &Roots, other_programs: &[PathBuf], traced_keys: &[String]) -> io::Result<Report> {
    let created = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let id = format!("{created}-{}", std::process::id());
    let dir = batches_dir()?.join(&id);
    fs::create_dir_all(&dir)?;
    let mut removal =
        Removal { id: id.clone(), dir: dir.clone(), roots, others: other_programs, traced_keys, items: Vec::new(), pending: Vec::new(), batch_dirs: Vec::new() };

    let mut log = Vec::new();
    for target in targets {
        let result = match target {
            Target::Dir { path } | Target::File { path } => removal.path(path),
            Target::RegKey { key } => removal.reg_key(key),
            Target::RegValue { key, name } => removal.reg_value(key, name),
            Target::Service { name } => removal.service(name),
        };
        let (ok, message) = match result {
            Ok(m) => (true, m),
            Err(e) => (false, e.to_string()),
        };
        log.push(LogEntry { target: target.clone(), ok, message });
        // The manifest is rewritten after every item so a crash mid-way still leaves an undo record.
        let manifest = Manifest { label: label.into(), created, items: removal.items.clone() };
        fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    }

    let batch = if removal.items.is_empty() {
        let _ = fs::remove_dir_all(&dir);
        None
    } else {
        Some(id)
    };
    Ok(Report { batch, log, pending_reboot: removal.pending })
}

fn read_manifest(id: &str) -> io::Result<Manifest> {
    if !valid_id(id) {
        return Err(io::Error::other("invalid batch id"));
    }
    Ok(serde_json::from_slice(&fs::read(batches_dir()?.join(id).join("manifest.json"))?)?)
}

/// Quarantine folders (one per volume) that hold this batch's files.
fn payload_dirs(manifest: &Manifest) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = manifest
        .items
        .iter()
        .filter_map(|i| match i {
            // stored = <base>\<batch id>\<item number>\<name>
            Stored::Path { stored, .. } => Some(Path::new(stored).parent()?.parent()?.to_path_buf()),
            _ => None,
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn list() -> Vec<BatchInfo> {
    let mut out: Vec<BatchInfo> = batches_dir()
        .and_then(fs::read_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let m = read_manifest(&id).ok()?;
            let size_kb = payload_dirs(&m).iter().map(|d| paths::dir_size_kb(d)).sum();
            Some(BatchInfo { id, label: m.label, created: m.created, items: m.items.len(), size_kb })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.created));
    out
}

/// Puts a batch back. Existing files and folders are never overwritten; items that could
/// not be restored stay in quarantine and are named in the returned messages.
pub fn restore(id: &str) -> io::Result<Vec<String>> {
    let manifest = read_manifest(id)?;
    let dir = batches_dir()?.join(id);
    let mut problems = Vec::new();
    let mut remaining = Vec::new();
    for item in &manifest.items {
        let result = match item {
            Stored::Path { original, stored } => {
                if fs::symlink_metadata(original).is_ok() {
                    Err(io::Error::other("something already exists at the original location"))
                } else {
                    fs::create_dir_all(Path::new(original).parent().unwrap_or(Path::new("")))
                        .and_then(|()| fs::rename(stored, original))
                }
            }
            Stored::RegKey { file, .. } | Stored::Service { file, .. } => {
                run("reg.exe", &["import", &dir.join(file).to_string_lossy(), "/reg:64"])
            }
            Stored::RegValue { key, name, vtype, data } => split_key(key).and_then(|(root, path)| {
                let (k, _) = root.create_subkey_with_flags(path, KEY_ALL_ACCESS | KEY_WOW64_64KEY)?;
                k.set_raw_value(name, &RegValue { vtype: reg_type(*vtype), bytes: data.as_slice().into() })
            }),
        };
        if let Err(e) = result {
            let what = match item {
                Stored::Path { original, .. } => original.clone(),
                Stored::RegKey { key, .. } => key.clone(),
                Stored::RegValue { key, name, .. } => format!("{key} -> {name}"),
                Stored::Service { name, .. } => format!("service {name}"),
            };
            problems.push(format!("{what}: {e}"));
            remaining.push(item.clone());
        }
    }
    if remaining.is_empty() {
        purge(id)?;
    } else {
        let left = Manifest { items: remaining, ..manifest };
        fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&left)?)?;
    }
    Ok(problems)
}

/// Deletes everything under `path` that lets itself be deleted and returns what is left,
/// children before the folders that hold them. A file some program has loaded is the usual reason.
fn delete_what_can_go(path: &Path, left: &mut Vec<PathBuf>) {
    let is_dir = fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    if is_dir {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            delete_what_can_go(&entry.path(), left);
        }
    }
    let gone = if is_dir { fs::remove_dir(path) } else { fs::remove_file(path).or_else(|_| fs::remove_dir(path)) };
    if gone.is_err_and(|e| e.kind() != io::ErrorKind::NotFound) {
        left.push(path.to_path_buf());
    }
}

/// Deletes a batch for good. Files a running program still has loaded cannot be deleted now;
/// Windows deletes those at the next restart, and they are returned.
pub fn purge(id: &str) -> io::Result<Vec<String>> {
    let manifest = read_manifest(id)?;
    let mut left = Vec::new();
    for payload in payload_dirs(&manifest) {
        delete_what_can_go(&payload, &mut left);
        // The per-volume quarantine folder itself goes once its last batch is gone.
        let _ = payload.parent().map(fs::remove_dir);
    }
    // The batch's record is only dropped once everything is gone or will be: otherwise
    // nothing would remember the files that stayed.
    if let Some(stuck) = left.iter().find(|path| !delete_on_reboot(path)) {
        return Err(io::Error::other(format!("in use and could not be scheduled for deletion: {}", stuck.display())));
    }
    // Asked for after the batch folders, so the per-volume folder goes too if this was its last
    // batch; Windows leaves a folder that is not empty by then.
    for quarantine in left.iter().filter_map(|p| payload_dirs(&manifest).into_iter().find(|d| d == p)?.parent().map(Path::to_path_buf)) {
        delete_on_reboot(&quarantine);
    }
    fs::remove_dir_all(batches_dir()?.join(id))?;
    Ok(left.iter().filter(|p| !p.is_dir()).map(|p| p.to_string_lossy().into_owned()).collect())
}

/// Drops batches older than `days`. Returns how many were removed.
pub fn purge_older_than(days: u64) -> usize {
    let cutoff = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).saturating_sub(days * 86_400);
    list().iter().filter(|b| b.created < cutoff && purge(&b.id).is_ok()).count()
}

#[cfg(test)]
mod tests;
