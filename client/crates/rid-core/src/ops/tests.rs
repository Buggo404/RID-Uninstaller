use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use super::*;
use crate::testutil::Sandbox;

thread_local! {
    /// Redirects quarantine storage into the test's sandbox (each test runs on its own thread).
    pub(super) static BASE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn sandbox(name: &str) -> Sandbox {
    let s = Sandbox::new(name);
    BASE.with(|b| *b.borrow_mut() = Some(s.0.join("quarantine")));
    s
}

fn dir(path: &std::path::Path) -> Target {
    Target::Dir { path: path.to_string_lossy().into_owned() }
}

#[test]
fn removed_files_come_back_on_restore() {
    let s = sandbox("roundtrip");
    let roots = s.roots();
    let app = s.dir(r"Apps\Thing");
    let deep = s.file(r"Apps\Thing\bin\x64\thing.exe");
    let data = s.file(r"Profile\Roaming\Vendor\Thing\settings.json");
    let targets = [dir(&app), dir(data.parent().unwrap())];

    let report = remove("Thing", &targets, &roots, &[], &[]).unwrap();
    assert!(report.log.iter().all(|l| l.ok), "{:?}", report.log);
    assert!(report.pending_reboot.is_empty());
    assert!(!app.exists() && !data.exists());
    // The vendor folder was left empty by the removal and is tidied away.
    assert!(!s.0.join(r"Profile\Roaming\Vendor").exists());
    assert!(roots.roaming.exists());

    let id = report.batch.expect("something was removed");
    let listed = list();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].id.as_str(), listed[0].label.as_str(), listed[0].items), (id.as_str(), "Thing", 2));

    assert_eq!(restore(&id).unwrap(), Vec::<String>::new());
    assert_eq!(fs::read_to_string(&deep).unwrap(), r"Apps\Thing\bin\x64\thing.exe");
    assert_eq!(fs::read_to_string(&data).unwrap(), r"Profile\Roaming\Vendor\Thing\settings.json");
    assert!(list().is_empty());
}

#[test]
fn restore_never_overwrites_what_is_there_now() {
    let s = sandbox("nooverwrite");
    let roots = s.roots();
    let file = s.file(r"Apps\Thing\config.ini");
    let id = remove("Thing", &[dir(file.parent().unwrap())], &roots, &[], &[]).unwrap().batch.unwrap();

    // The program was reinstalled in the meantime.
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "new").unwrap();

    let problems = restore(&id).unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "new");
    assert_eq!(list().len(), 1, "the unrestored batch stays in quarantine");

    purge(&id).unwrap();
    assert!(list().is_empty());
    assert!(!s.0.join(r"quarantine\payload").exists());
}

/// The UI only proposes; a request for a protected folder is refused when it is carried out.
#[test]
fn protected_targets_are_refused_at_removal_time() {
    let s = sandbox("guard");
    let roots = s.roots();
    let other = s.dir(r"Apps\Suite\Other");
    let suite = other.parent().unwrap().to_path_buf();
    let targets = [dir(&roots.program_files[0]), dir(&roots.roaming), dir(&suite), Target::Dir { path: r"..\x".into() }];

    let report = remove("bad", &targets, &roots, &[other.clone()], &[]).unwrap();
    assert!(report.log.iter().all(|l| !l.ok), "{:?}", report.log);
    assert_eq!(report.batch, None);
    assert!(roots.program_files[0].exists() && roots.roaming.exists() && other.exists());
    assert!(list().is_empty());

    for key in [r"HKLM\SOFTWARE\Microsoft", r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip", r"HKCU\Software"] {
        let report = remove("bad", &[Target::RegKey { key: key.into() }], &roots, &[], &[]).unwrap();
        assert!(!report.log[0].ok, "{key}");
    }
    let report = remove("bad", &[Target::Service { name: "Tcpip".into() }], &roots, &[], &[]).unwrap();
    assert!(!report.log[0].ok, "a Windows service must be refused: {:?}", report.log);
}

#[test]
fn batch_ids_cannot_escape_the_quarantine_folder() {
    let _s = sandbox("ids");
    for id in ["", "..", r"..\..\Windows", "1/2", "abc", "1-2\\3"] {
        assert!(restore(id).is_err(), "{id}");
        assert!(purge(id).is_err(), "{id}");
    }
}

/// Uses a key of its own under HKCU\Software and deletes it afterwards.
#[test]
fn registry_keys_and_values_come_back_on_restore() {
    let s = sandbox("registry");
    let roots = s.roots();
    let name = format!("RidOpsTest{}", std::process::id());
    let key = format!(r"HKCU\Software\{name}");
    let software = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software").unwrap();
    let (k, _) = software.create_subkey(format!(r"{name}\Sub")).unwrap();
    k.set_value("text", &"héllo").unwrap();
    k.set_value("number", &42u32).unwrap();

    let report = remove("reg", &[Target::RegKey { key: key.clone() }], &roots, &[], &[]).unwrap();
    assert!(report.log[0].ok, "{:?}", report.log);
    assert!(software.open_subkey(&name).is_err(), "key should be gone");

    let problems = restore(&report.batch.unwrap()).unwrap();
    let restored = software.open_subkey(format!(r"{name}\Sub"));
    let values = restored.as_ref().map(|k| (k.get_value::<String, _>("text").ok(), k.get_value::<u32, _>("number").ok()));
    let _ = software.delete_subkey_all(&name);
    assert_eq!(problems, Vec::<String>::new());
    assert_eq!(values.unwrap(), (Some("héllo".to_string()), Some(42)));
}
