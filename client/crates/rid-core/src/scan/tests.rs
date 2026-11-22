use std::path::{Path, PathBuf};

use super::*;
use crate::testutil::Sandbox;

fn app(id: &str, name: &str) -> App {
    App { id: id.into(), name: name.into(), ..App::default() }
}

/// Folder findings only: registry findings depend on what this machine has installed.
fn dirs(found: &[Leftover]) -> Vec<(PathBuf, bool)> {
    found
        .iter()
        .filter_map(|l| match &l.target {
            Target::Dir { path } => Some((PathBuf::from(path), l.certain)),
            _ => None,
        })
        .collect()
}

/// Reported against Geek Uninstaller: an uninstaller sitting directly in Program Files made
/// the whole of Program Files a "leftover" (Hikvision iVMS-4200, 300 GB lost).
#[test]
fn uninstaller_in_program_files_does_not_offer_program_files() {
    let s = Sandbox::new("hik");
    let roots = s.roots();
    let mut a = app("HKLM:32:iVMS", "iVMS-4200 Client RidTest");
    a.uninstall_string = s.file(r"PF86\uninstall.exe").to_string_lossy().into_owned();
    assert_eq!(dirs(&scan(&a, &[a.clone()], &roots)), []);
}

#[test]
fn folder_inferred_from_uninstaller_is_certain_only_when_named_after_the_program() {
    let s = Sandbox::new("infer");
    let roots = s.roots();
    let mut named = app("HKLM:64:a", "RidTestEverything 1.2.1");
    named.uninstall_string = format!("\"{}\" /S", s.file(r"Apps\RidTestEverything\Uninstall.exe").display());
    assert_eq!(dirs(&scan(&named, &[named.clone()], &roots)), [(s.0.join(r"Apps\RidTestEverything"), true)]);

    let mut unnamed = app("HKLM:64:b", "RidTestEverything 1.2.1");
    unnamed.uninstall_string = s.file(r"Stuff\unins000.exe").to_string_lossy().into_owned();
    assert_eq!(dirs(&scan(&unnamed, &[unnamed.clone()], &roots)), [(s.0.join("Stuff"), false)]);
}

/// Reported against Uninstall Tool (Teams took Office with it) and Revo (other games listed).
#[test]
fn folder_holding_another_program_is_not_offered() {
    let s = Sandbox::new("suite");
    let roots = s.roots();
    let mut suite = app("HKLM:64:suite", "RidTestSuite");
    suite.install_location = s.dir("Suite").to_string_lossy().into_owned();
    let mut part = app("HKLM:64:part", "RidTestSuite Component");
    part.install_location = s.dir(r"Suite\Component").to_string_lossy().into_owned();
    let all = [suite.clone(), part.clone()];

    assert_eq!(dirs(&scan(&suite, &all, &roots)), []);
    // The component's own folder is offered, but not pre-selected: it sits inside another program.
    assert_eq!(dirs(&scan(&part, &all, &roots)), [(s.0.join(r"Suite\Component"), false)]);
}

/// Reported against Geek Uninstaller: Autodesk offered as a leftover of ToDesk.
#[test]
fn data_folders_match_by_whole_name() {
    let s = Sandbox::new("names");
    let roots = s.roots();
    s.dir(r"Profile\Roaming\RidTestToDesk");
    s.dir(r"Profile\Roaming\AutoRidTestToDesk");
    s.dir(r"Profile\Local\RidTestToDesk_Lite");
    s.dir(r"ProgramData\RidTestToDesk");
    let a = app("HKLM:64:todesk", "RidTestToDesk 4.7.2");
    let mut found = dirs(&scan(&a, &[a.clone()], &roots));
    found.sort();
    assert_eq!(found, [(s.0.join(r"Profile\Roaming\RidTestToDesk"), true), (s.0.join(r"ProgramData\RidTestToDesk"), true)]);
}

#[test]
fn name_shared_with_another_installed_program_is_not_preselected() {
    let s = Sandbox::new("shared");
    let roots = s.roots();
    s.dir(r"Profile\Roaming\RidTestPlayer");
    let a = app("HKLM:64:a", "RidTestPlayer 1.0");
    let b = app("HKCU:64:b", "RidTestPlayer 2.0");
    assert_eq!(dirs(&scan(&a, &[a.clone(), b], &roots)), [(s.0.join(r"Profile\Roaming\RidTestPlayer"), false)]);
}

/// Reported against Uninstall Tool: the shared `ProgramData\NVIDIA\NGX` offered for another NVIDIA tool.
#[test]
fn publisher_folder_yields_only_the_program_subfolder() {
    let s = Sandbox::new("vendor");
    let roots = s.roots();
    s.dir(r"ProgramData\RidTestVendor\NGX");
    s.dir(r"ProgramData\RidTestVendor\Shared Runtime");
    s.dir(r"ProgramData\RidTestVendor\Nsight Graphics");
    let mut a = app("HKLM:64:nsight", "RidTestVendor Nsight Graphics 2024.1");
    a.publisher = "RidTestVendor Corporation".into();
    assert_eq!(dirs(&scan(&a, &[a.clone()], &roots)), [(s.0.join(r"ProgramData\RidTestVendor\Nsight Graphics"), true)]);
}

#[test]
fn start_menu_folder_with_foreign_files_is_kept() {
    let s = Sandbox::new("menu");
    let roots = s.roots();
    let mut a = app("HKLM:64:menu", "RidTestMenuApp");
    a.install_location = s.dir(r"Apps\RidTestMenuApp").to_string_lossy().into_owned();
    // Not shortcuts into the program, so neither the files nor their folder are leftovers.
    s.file(r"Profile\Roaming\Microsoft\Windows\Start Menu\Programs\Shared Group\Other.lnk");
    s.file(r"Profile\Roaming\Microsoft\Windows\Start Menu\Programs\Shared Group\readme.txt");
    assert_eq!(dirs(&scan(&a, &[a.clone()], &roots)), [(s.0.join(r"Apps\RidTestMenuApp"), true)]);
    assert!(scan(&a, &[a.clone()], &roots).iter().all(|l| !matches!(l.target, Target::File { .. })));
}

/// A folder holding the user's own documents is offered but never pre-selected.
#[test]
fn folders_with_user_documents_are_not_preselected() {
    let s = Sandbox::new("docs");
    let roots = s.roots();
    let mut a = app("HKLM:64:docs", "RidTestWriter");
    a.install_location = s.dir(r"Apps\RidTestWriter").to_string_lossy().into_owned();
    s.file(r"Apps\RidTestWriter\writer.exe");
    s.file(r"Profile\Roaming\RidTestWriter\settings.json");
    s.file(r"Profile\Roaming\RidTestWriter\Drafts\Thesis.DOCX");
    let found = scan(&a, &[a.clone()], &roots);
    let by_path = |end: &str| found.iter().find(|l| matches!(&l.target, Target::Dir { path } if path.ends_with(end))).unwrap();
    assert_eq!((by_path(r"Apps\RidTestWriter").certain, by_path(r"Apps\RidTestWriter").documents), (true, 0));
    assert_eq!((by_path(r"Roaming\RidTestWriter").certain, by_path(r"Roaming\RidTestWriter").documents), (false, 1));
}

#[test]
fn command_lines_are_matched_against_the_program_folder() {
    let dir = norm(Path::new(r"C:\Program Files\App")).unwrap();
    assert!(runs_from(r#""C:\Program Files\App\app.exe" --tray"#, &dir));
    assert!(runs_from(r"C:\PROGRAM FILES\App\bin\svc.exe", &dir));
    assert!(runs_from(r"\??\C:\Program Files\App\drv.sys", &dir));
    assert!(!runs_from(r"C:\Program Files\App2\app.exe", &dir));
    assert!(!runs_from(r"C:\Program Files\App", &dir));
    assert!(!runs_from(r"\SystemRoot\System32\drivers\x.sys", &dir));
}
