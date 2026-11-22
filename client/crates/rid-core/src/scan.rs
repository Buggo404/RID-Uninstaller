//! Finds what a program left behind. Every finding is backed by evidence that ties it to
//! this one program, and has already passed the `safety` checks.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

use crate::inventory::App;
use crate::safety::{self, Roots, norm, within};
use crate::{cmdline, links, names};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Dir { path: String },
    File { path: String },
    /// `HKLM\...` or `HKCU\...`, 64-bit view, `WOW6432Node` spelled out.
    RegKey { key: String },
    RegValue { key: String, name: String },
    Service { name: String },
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// The folder the installer registered as the program's location.
    InstallLocation,
    /// The folder holding the program's uninstaller.
    UninstallerFolder,
    /// A shortcut pointing into the program's folder, or a Start Menu folder holding only those.
    Shortcut,
    /// Named exactly like the program.
    NameMatch,
    /// Named like the program, inside a folder or key named like its publisher.
    PublisherAndName,
    /// The program's own entry in the installed-programs list.
    UninstallEntry,
    /// An App Paths entry, autostart entry or service that runs a file from the program's folder.
    RunsFromFolder,
    /// Windows Installer bookkeeping for this exact product code.
    InstallerRecord,
    /// Recorded by the install tracer while the program was being installed.
    Traced,
}

#[derive(Serialize, Clone, Debug)]
pub struct Leftover {
    pub target: Target,
    pub evidence: Evidence,
    /// Selected by default. `false` when the evidence leaves room for doubt.
    pub certain: bool,
    pub size_kb: u64,
    /// Files inside that look like the user's own work. Any at all and the item is not
    /// selected by default, whatever the evidence.
    pub documents: u32,
}

/// File types people create and installers practically never ship.
const DOCUMENTS: [&str; 13] =
    ["doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "psd", "dwg", "kdbx", "pst"];

fn is_document(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|x| DOCUMENTS.contains(&x.to_ascii_lowercase().as_str()))
}

/// Bytes and document count under `dir`. Links are not followed.
fn measure(dir: &Path) -> (u64, u32) {
    let Ok(entries) = fs::read_dir(dir) else { return (0, 0) };
    entries.flatten().fold((0, 0), |(bytes, docs), e| match e.file_type() {
        Ok(t) if t.is_symlink() => (bytes, docs),
        Ok(t) if t.is_dir() => {
            let (b, d) = measure(&e.path());
            (bytes + b, docs + d)
        }
        Ok(_) => (bytes + e.metadata().map(|m| m.len()).unwrap_or(0), docs + u32::from(is_document(&e.path()))),
        Err(_) => (bytes, docs),
    })
}

fn leftover(target: Target, evidence: Evidence, certain: bool) -> Leftover {
    let (bytes, documents) = match &target {
        Target::Dir { path } => measure(Path::new(path)),
        Target::File { path } => (fs::metadata(path).map(|m| m.len()).unwrap_or(0), u32::from(is_document(Path::new(path)))),
        _ => (0, 0),
    };
    Leftover { target, evidence, certain: certain && documents == 0, size_kb: bytes.div_ceil(1024), documents }
}

fn uninstaller_dir(app: &App) -> Option<PathBuf> {
    if app.is_msi {
        return None; // MsiExec.exe lives in System32
    }
    Some(Path::new(cmdline::exe_of(&app.uninstall_string)?).parent()?.to_path_buf())
}

/// Folders the installed programs live in, except those being removed. Nothing at or above
/// these is ever a leftover.
pub fn other_program_dirs(removing: &[&str], all: &[App]) -> Vec<PathBuf> {
    all.iter()
        .filter(|o| !removing.contains(&o.id.as_str()))
        .flat_map(|o| [(!o.install_location.is_empty()).then(|| PathBuf::from(&o.install_location)), uninstaller_dir(o)])
        .flatten()
        .collect()
}

/// The folder `app` is installed in, if it is one Rid would be allowed to remove.
pub fn program_folder(app: &App, all: &[App], roots: &Roots) -> Option<PathBuf> {
    let others = other_program_dirs(&[&app.id], all);
    program_dir(app, roots, &others, &names::name_keys(&app.name)).map(|(dir, ..)| dir)
}

/// The program's own folder, with whether that is certain enough to select by default.
fn program_dir(app: &App, roots: &Roots, others: &[PathBuf], keys: &[String]) -> Option<(PathBuf, Evidence, bool)> {
    let (dir, evidence) = if !app.install_location.is_empty() {
        (PathBuf::from(&app.install_location), Evidence::InstallLocation)
    } else {
        (uninstaller_dir(app)?, Evidence::UninstallerFolder)
    };
    let dir = safety::resolve(&dir);
    if !dir.is_dir() || safety::check_path(&dir, roots, others).is_err() {
        return None;
    }
    let p = norm(&dir)?;
    let inside_other = others.iter().filter_map(|o| norm(o)).any(|o| within(&p, &o));
    // A folder inferred from the uninstaller must also be named after the program.
    let named = dir.file_name().is_some_and(|n| names::is_program(&n.to_string_lossy(), keys));
    Some((dir, evidence, !inside_other && (evidence == Evidence::InstallLocation || named)))
}

fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect()
}

/// Shortcuts into `program` (a folder in `norm` form) or belonging to the Windows Installer
/// `product`, found under `dir`. Returns whether everything in `dir` was one.
fn shortcuts(dir: &Path, program: Option<&str>, product: Option<&str>, depth: u32, found: &mut Vec<PathBuf>) -> bool {
    let ours = |lnk: &Path| {
        program.is_some_and(|p| links::target(lnk).and_then(|t| norm(&t)).is_some_and(|t| within(&t, p)))
            || product.is_some_and(|code| links::msi_product(lnk).is_some_and(|c| c.eq_ignore_ascii_case(code)))
    };
    let Ok(entries) = fs::read_dir(dir) else { return false };
    let mut all_ours = true;
    let mut any = false;
    for e in entries.flatten() {
        let path = e.path();
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if is_dir && depth > 0 {
            let mut inner = Vec::new();
            if shortcuts(&path, program, product, depth - 1, &mut inner) {
                found.push(path); // the whole folder stands for its shortcuts
                any = true;
            } else {
                found.append(&mut inner);
                all_ours = false;
            }
        } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) && ours(&path) {
            found.push(path);
            any = true;
        } else if !e.file_name().eq_ignore_ascii_case("desktop.ini") {
            all_ours = false;
        }
    }
    any && all_ours
}

/// What an install recording says the program created, limited to what is still there and
/// what the safety rules allow to be removed.
pub fn traced(items: &[Target], roots: &Roots, others: &[PathBuf]) -> Vec<Leftover> {
    items
        .iter()
        .filter(|item| match item {
            Target::Dir { path } | Target::File { path } => {
                let path = safety::resolve(Path::new(path));
                fs::symlink_metadata(&path).is_ok() && safety::check_path(&path, roots, others).is_ok()
            }
            Target::RegKey { key } => {
                safety::check_key_traced(key).is_ok() && key.split_once('\\').is_some_and(|(hive, path)| open(hive, path).is_some())
            }
            Target::RegValue { key, name } => {
                safety::check_value(key).is_ok()
                    && key.split_once('\\').and_then(|(hive, path)| open(hive, path)).is_some_and(|k| k.get_raw_value(name).is_ok())
            }
            Target::Service { .. } => false,
        })
        .map(|item| leftover(item.clone(), Evidence::Traced, true))
        .collect()
}

/// Is `inner` the same item as `outer`, or inside the folder or key `outer` names?
pub fn covered_by(inner: &Target, outer: &Target) -> bool {
    match (inner, outer) {
        (Target::Dir { path: i } | Target::File { path: i }, Target::Dir { path: o } | Target::File { path: o }) => {
            norm(Path::new(i)).zip(norm(Path::new(o))).is_some_and(|(i, o)| within(&i, &o))
        }
        (Target::RegKey { key: i } | Target::RegValue { key: i, .. }, Target::RegKey { key: o }) => {
            let (i, o) = (i.to_lowercase(), o.to_lowercase());
            i == o || i.strip_prefix(&o).is_some_and(|rest| rest.starts_with('\\'))
        }
        _ => inner == outer,
    }
}

/// The installed program a shortcut belongs to: the one whose folder holds the shortcut's
/// target, or, for a Windows Installer shortcut, the product it advertises.
pub fn owner_of_shortcut<'a>(lnk: &Path, all: &'a [App], roots: &Roots) -> Option<&'a App> {
    let _com = links::Com::init();
    if let Some(code) = links::msi_product(lnk) {
        return all.iter().find(|a| a.is_msi && a.id.rsplit(':').next().is_some_and(|k| k.eq_ignore_ascii_case(&code)));
    }
    let target = norm(&links::target(lnk)?)?;
    all.iter().find(|app| program_folder(app, all, roots).and_then(|dir| norm(&dir)).is_some_and(|dir| within(&target, &dir)))
}

pub fn scan(app: &App, all: &[App], roots: &Roots) -> Vec<Leftover> {
    let keys = names::name_keys(&app.name);
    let publisher = names::publisher_key(&app.publisher);
    let others = other_program_dirs(&[&app.id], all);
    // A name shared with another installed program proves nothing about whose data it is.
    let unique = !all.iter().any(|o| o.id != app.id && names::name_keys(&o.name).iter().any(|k| keys.contains(k)));
    let mut out = Vec::new();

    let program = program_dir(app, roots, &others, &keys);
    let program_norm = program.as_ref().and_then(|(dir, ..)| norm(dir));
    if let Some((dir, evidence, certain)) = &program {
        out.push(leftover(Target::Dir { path: dir.to_string_lossy().into_owned() }, *evidence, *certain));
    }

    let product = app.is_msi.then(|| app.id.rsplit(':').next().unwrap_or_default());
    if program_norm.is_some() || product.is_some() {
        let _com = links::Com::init();
        let mut found = Vec::new();
        for menu in &roots.start_menus {
            shortcuts(menu, program_norm.as_deref(), product, 3, &mut found);
        }
        for desktop in &roots.desktops {
            shortcuts(desktop, program_norm.as_deref(), product, 0, &mut found);
        }
        for path in found {
            let path = safety::resolve(&path);
            if safety::check_path(&path, roots, &others).is_ok() {
                let p = path.to_string_lossy().into_owned();
                let target = if path.is_dir() { Target::Dir { path: p } } else { Target::File { path: p } };
                out.push(leftover(target, Evidence::Shortcut, true));
            }
        }
    }

    let mut data_dir = |path: PathBuf, evidence: Evidence| {
        let path = safety::resolve(&path);
        let already = norm(&path).zip(program_norm.as_ref()).is_some_and(|(p, prog)| within(&p, prog));
        if !already && safety::check_path(&path, roots, &others).is_ok() {
            out.push(leftover(Target::Dir { path: path.to_string_lossy().into_owned() }, evidence, unique));
        }
    };
    for root in [&roots.roaming, &roots.local, &roots.local_low, &roots.program_data, &roots.local.join("Programs")] {
        for (name, path) in subdirs(root) {
            if names::is_program(&name, &keys) {
                data_dir(path, Evidence::NameMatch);
            } else if publisher.as_deref().is_some_and(|p| names::key(&name) == p) {
                for (child, child_path) in subdirs(&path) {
                    if names::is_program_under_publisher(&child, &keys, publisher.as_deref().unwrap_or_default()) {
                        data_dir(child_path, Evidence::PublisherAndName);
                    }
                }
            }
        }
    }

    out.extend(scan_registry(app, &keys, publisher.as_deref(), unique, program_norm.as_deref()));
    out
}

/// Does a command line or image path start with a file inside `dir` (in `norm` form)?
fn runs_from(command: &str, dir: &str) -> bool {
    let c = command.trim().trim_start_matches('"').trim_start_matches(r"\??\").replace('/', "\\").to_lowercase();
    c.strip_prefix(dir).is_some_and(|rest| rest.starts_with('\\'))
}

use crate::msi::packed_product_code;

fn open(hive: &str, path: &str) -> Option<RegKey> {
    let root = if hive == "HKLM" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
    RegKey::predef(root).open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY).ok()
}

fn scan_registry(app: &App, keys: &[String], publisher: Option<&str>, unique: bool, program: Option<&str>) -> Vec<Leftover> {
    let mut out = Vec::new();
    let mut key = |key: String, evidence: Evidence, certain: bool| {
        // The uninstaller may already have removed it; only what is still there is a leftover.
        let exists = key.split_once('\\').is_some_and(|(hive, path)| open(hive, path).is_some());
        if exists && safety::check_key(&key).is_ok() {
            out.push(leftover(Target::RegKey { key }, evidence, certain));
        }
    };

    if let Some(path) = app.reg_path.strip_prefix("HKEY_LOCAL_MACHINE") {
        key(format!("HKLM{path}"), Evidence::UninstallEntry, true);
    } else if let Some(path) = app.reg_path.strip_prefix("HKEY_CURRENT_USER") {
        key(format!("HKCU{path}"), Evidence::UninstallEntry, true);
    }

    for hive in ["HKCU", "HKLM"] {
        for software in ["SOFTWARE", r"SOFTWARE\WOW6432Node"] {
            let Some(root) = open(hive, software) else { continue };
            for name in root.enum_keys().flatten() {
                if names::is_program(&name, keys) {
                    key(format!(r"{hive}\{software}\{name}"), Evidence::NameMatch, unique);
                } else if publisher.is_some_and(|p| names::key(&name) == p) {
                    let Some(vendor) = open(hive, &format!(r"{software}\{name}")) else { continue };
                    for child in vendor.enum_keys().flatten() {
                        if names::is_program_under_publisher(&child, keys, publisher.unwrap_or_default()) {
                            key(format!(r"{hive}\{software}\{name}\{child}"), Evidence::PublisherAndName, unique);
                        }
                    }
                }
            }
        }
    }

    if app.is_msi
        && let Some(code) = app.id.rsplit(':').next().and_then(packed_product_code)
    {
        let mut records = vec![
            format!(r"HKLM\SOFTWARE\Classes\Installer\Products\{code}"),
            format!(r"HKLM\SOFTWARE\Classes\Installer\Features\{code}"),
            format!(r"HKCU\Software\Microsoft\Installer\Products\{code}"),
            format!(r"HKCU\Software\Microsoft\Installer\Features\{code}"),
        ];
        let user_data = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData";
        for sid in open("HKLM", user_data).iter().flat_map(|k| k.enum_keys().flatten()) {
            records.push(format!(r"HKLM\{user_data}\{sid}\Products\{code}"));
        }
        for record in records {
            key(record, Evidence::InstallerRecord, true);
        }
    }

    let Some(program) = program else { return out };
    let current_version = r"Microsoft\Windows\CurrentVersion";
    for hive in ["HKCU", "HKLM"] {
        for software in ["SOFTWARE", r"SOFTWARE\WOW6432Node"] {
            let app_paths = format!(r"{software}\{current_version}\App Paths");
            for (name, entry) in open(hive, &app_paths)
                .iter()
                .flat_map(|k| k.enum_keys().flatten().filter_map(|n| Some((n.clone(), k.open_subkey(&n).ok()?))))
                .collect::<Vec<_>>()
            {
                if entry.get_value::<String, _>("").is_ok_and(|exe| runs_from(&exe, program)) {
                    let k = format!(r"{hive}\{app_paths}\{name}");
                    if safety::check_key(&k).is_ok() {
                        out.push(leftover(Target::RegKey { key: k }, Evidence::RunsFromFolder, true));
                    }
                }
            }
            for run in ["Run", "RunOnce"] {
                let run_key = format!(r"{software}\{current_version}\{run}");
                let Some(k) = open(hive, &run_key) else { continue };
                for (name, value) in k.enum_values().flatten() {
                    let full = format!(r"{hive}\{run_key}");
                    if runs_from(&value.to_string(), program) && safety::check_value(&full).is_ok() {
                        out.push(leftover(Target::RegValue { key: full, name }, Evidence::RunsFromFolder, true));
                    }
                }
            }
        }
    }
    let services = r"SYSTEM\CurrentControlSet\Services";
    for name in open("HKLM", services).iter().flat_map(|k| k.enum_keys().flatten()) {
        let image: Option<String> = open("HKLM", &format!(r"{services}\{name}")).and_then(|k| k.get_value("ImagePath").ok());
        if image.is_some_and(|i| runs_from(&i, program)) {
            out.push(leftover(Target::Service { name }, Evidence::RunsFromFolder, true));
        }
    }
    out
}

#[cfg(test)]
mod tests;
