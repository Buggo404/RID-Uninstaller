#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use rid_core::inventory::{self, App, Inventory};
use rid_core::ops::{self, BatchInfo, Report};
use rid_core::procs::{self, Proc};
use rid_core::safety::Roots;
use rid_core::scan::{self, Leftover, Target};
use rid_core::{ai, driver, facts, icons, paths, rules, shell, startup, trace, traced, uninstall, update};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Emitter, Manager};
use tauri::http::{Response, StatusCode};

type CmdResult<T = ()> = Result<T, String>;

fn err(e: impl ToString) -> String {
    e.to_string()
}

/// The window has its own words for a driver Windows refuses for its signature.
fn driver_err(e: std::io::Error) -> String {
    if e.raw_os_error() == Some(driver::UNSIGNED) { "driver-unsigned".into() } else { err(e) }
}

#[derive(Default)]
struct AppState {
    /// The list as last shown. Removal works from this snapshot, because a program that has
    /// just been uninstalled is no longer in the registry but its leftovers still need its details.
    apps: Mutex<Vec<App>>,
    cancel: AtomicBool,
    /// Result of the last install recording, until the user decides what to do with it.
    pending_trace: Mutex<Vec<Target>>,
    /// The list read while the window was still starting, for the first `list_apps`.
    prefetch: Mutex<Option<std::thread::JoinHandle<Inventory>>>,
    /// The newest size-measuring pass; older ones stop when they see a newer number.
    sizing: AtomicU64,
}

type State<'a> = tauri::State<'a, AppState>;

fn known(state: &AppState, ids: &[String]) -> CmdResult<Vec<App>> {
    let found: CmdResult<Vec<App>> = {
        let apps = state.apps.lock().map_err(err)?;
        ids.iter()
            .map(|id| apps.iter().find(|a| &a.id == id).cloned().ok_or_else(|| format!("unknown program: {id}")))
            .collect()
    };
    // The list is shown before Windows Installer's locations are known; work on a program needs them.
    let mut found = found?;
    inventory::complete(&mut found);
    Ok(found)
}

#[tauri::command(async)]
fn list_apps(state: State) -> CmdResult<Inventory> {
    let prefetched = state.prefetch.lock().map_err(err)?.take().and_then(|reader| reader.join().ok());
    let mut inv = prefetched.unwrap_or_else(|| {
        inventory::warm_up();
        inventory::list()
    });
    for record in traced::list() {
        match inv.apps.iter_mut().find(|a| a.id == record.app_id) {
            Some(app) => app.traced = true,
            // An installation that registered no uninstall entry: listed from its recording alone.
            None if record.app_id.starts_with("TRACE:") => {
                inv.apps.push(App { id: record.app_id, name: record.name, traced: true, ..App::default() });
            }
            None => {}
        }
    }
    inv.apps.sort_by_cached_key(|a| a.name.to_lowercase());
    *state.apps.lock().map_err(err)? = inv.apps.clone();
    Ok(inv)
}

/// Install locations only Windows Installer's component table knows, as (id, folder). Reading
/// that table takes seconds, so the window shows the list first and asks for these afterwards.
#[tauri::command(async)]
fn msi_locations(state: State) -> CmdResult<Vec<(String, String)>> {
    let mut apps: Vec<App> =
        state.apps.lock().map_err(err)?.iter().filter(|a| a.is_msi && a.install_location.is_empty()).cloned().collect();
    inventory::complete(&mut apps);
    let found: Vec<(String, String)> =
        apps.into_iter().filter(|a| !a.install_location.is_empty()).map(|a| (a.id, a.install_location)).collect();
    let mut shown = state.apps.lock().map_err(err)?;
    for (id, folder) in &found {
        if let Some(app) = shown.iter_mut().find(|a| &a.id == id && a.install_location.is_empty()) {
            app.install_location = folder.clone();
        }
    }
    Ok(found)
}

/// Measures the folders of `items` (id, folder) on several threads, reporting each as a
/// `dir-size` event the moment it is known. A call with a higher `pass` stops this one.
#[tauri::command(async)]
fn dir_sizes(app: tauri::AppHandle, state: State, pass: u64, items: Vec<(String, String)>) {
    let newest = &state.sizing;
    newest.fetch_max(pass, Ordering::Relaxed);
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                while let Some((id, folder)) = items.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if newest.load(Ordering::Relaxed) > pass {
                        break;
                    }
                    let _ = app.emit("dir-size", (id, paths::dir_size_kb(Path::new(folder))));
                }
            });
        }
    });
}

#[tauri::command(async)]
fn driver_status() -> driver::Status {
    driver::status()
}

/// The Microsoft-signed driver, when the build was given one (RID_DRIVER, see build.rs).
#[cfg(embedded_driver)]
const DRIVER: Option<&[u8]> = Some(include_bytes!(env!("RID_DRIVER_FILE")));
#[cfg(not(embedded_driver))]
const DRIVER: Option<&[u8]> = None;

/// Installs the driver carried in the program; a build without one looks for the file beside the executable.
#[tauri::command(async)]
fn driver_install() -> CmdResult {
    let image = match DRIVER {
        Some(bytes) => {
            // Unpacked where only administrators can write: what gets installed is what was shipped.
            let file = paths::secure_dir().map_err(err)?.join("RidMonitor.sys");
            fs::write(&file, bytes).map_err(err)?;
            file
        }
        None => std::env::current_exe().map_err(err)?.with_file_name("RidMonitor.sys"),
    };
    driver::install(&image).map_err(driver_err)
}

#[tauri::command(async)]
fn driver_remove() -> CmdResult {
    driver::remove().map_err(err)
}

#[derive(Serialize)]
struct TraceResult {
    items: Vec<Target>,
    /// Events the driver had to drop. Non-zero means the recording is incomplete.
    dropped: u32,
    /// Programs that appeared in the installed list during the recording.
    new_apps: Vec<App>,
}

/// Runs a setup file under the tracer. Events are streamed to the window as `trace-event`.
#[tauri::command(async)]
fn trace_run(app: tauri::AppHandle, state: State, path: String, arguments: String) -> CmdResult<TraceResult> {
    if !Path::new(&path).is_file() {
        return Err(format!("no such file: {path}"));
    }
    driver::load().map_err(driver_err)?;
    let is_msi = Path::new(&path).extension().is_some_and(|x| x.eq_ignore_ascii_case("msi"));
    let command = match is_msi {
        true => format!("msiexec.exe /i \"{path}\" {arguments}"),
        false => format!("\"{path}\" {arguments}"),
    };
    let before: Vec<String> = inventory::list().apps.into_iter().map(|a| a.id).collect();
    state.cancel.store(false, Ordering::Relaxed);
    // Windows Installer works in its service process, also when a setup.exe merely wraps an MSI.
    let recording = trace::record(command.trim(), true, &state.cancel, |event| {
        let _ = app.emit("trace-event", event);
    })
    .map_err(err)?;
    let items = traced::reduce(&recording.events);
    *state.pending_trace.lock().map_err(err)? = items.clone();
    let new_apps = inventory::list().apps.into_iter().filter(|a| !before.contains(&a.id)).collect();
    Ok(TraceResult { items, dropped: recording.dropped, new_apps })
}

/// Keeps the last recording: for `app_id`, or as a list entry of its own named `custom_name`.
#[tauri::command(async)]
fn trace_save(state: State, app_id: Option<String>, custom_name: Option<String>, dependents: Vec<String>, append: bool) -> CmdResult {
    let items = std::mem::take(&mut *state.pending_trace.lock().map_err(err)?);
    let (id, name) = match (app_id, custom_name) {
        (Some(id), _) => {
            let name = inventory::list().apps.into_iter().find(|a| a.id == id).map(|a| a.name).unwrap_or_default();
            (id, name)
        }
        (None, Some(name)) if !name.trim().is_empty() => (format!("TRACE:{}", name.trim()), name.trim().to_string()),
        _ => return Err("nothing to attach the recording to".into()),
    };
    if append {
        return traced::append(&id, items).map_err(err);
    }
    if id.starts_with("TRACE:") && traced::load(&id).is_some() {
        return Err(format!("'{name}' already exists in the list, choose another name"));
    }
    traced::save(&traced::Record { app_id: id, name, created: traced::now(), items, dependents }).map_err(err)
}

/// Undoes the installation just recorded: removes everything it created.
#[tauri::command(async)]
fn trace_revert(state: State) -> CmdResult<Report> {
    let items = std::mem::take(&mut *state.pending_trace.lock().map_err(err)?);
    let keys: Vec<String> = items
        .iter()
        .filter_map(|i| match i {
            Target::RegKey { key } => Some(key.clone()),
            _ => None,
        })
        .collect();
    let others = scan::other_program_dirs(&[], &inventory::list_full().apps);
    // The freshly installed program is in `others`; its own folder is refused, which is why
    // reverting is offered only when the installation registered nothing.
    ops::remove("Reverted installation", &items, &Roots::from_system(), &others, &keys).map_err(err)
}

#[tauri::command(async)]
fn trace_get(id: String) -> Option<traced::Record> {
    traced::load(&id)
}

#[tauri::command(async)]
fn trace_forget(id: String) -> CmdResult {
    traced::forget(&id).map_err(err)
}

#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Launch {
    /// `--trace <setup file>`
    Trace { path: String },
    /// `--uninstall-shortcut <file.lnk>`: the program found for it, if any.
    Uninstall { app_id: Option<String>, shortcut: String },
    /// `--startup`
    Startup,
    /// `--store-apps`
    StoreApps,
}

/// What this process was started for.
#[tauri::command(async)]
fn launch_request(state: State) -> CmdResult<Option<Launch>> {
    launch(&std::env::args().skip(1).collect::<Vec<_>>(), &state)
}

/// What a command line asks for (Explorer's context menu starts Rid this way).
fn launch(args: &[String], state: &AppState) -> CmdResult<Option<Launch>> {
    Ok(match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["--trace", path] => Some(Launch::Trace { path: path.into() }),
        ["--uninstall-shortcut", shortcut] => {
            let apps = state.apps.lock().map_err(err)?;
            let owner = scan::owner_of_shortcut(Path::new(shortcut), &apps, &Roots::from_system());
            Some(Launch::Uninstall { app_id: owner.map(|a| a.id.clone()), shortcut: shortcut.into() })
        }
        ["--startup"] => Some(Launch::Startup),
        ["--store-apps"] => Some(Launch::StoreApps),
        _ => None,
    })
}

#[tauri::command]
fn context_menu_enabled() -> bool {
    shell::context_menu_enabled()
}

#[tauri::command]
fn set_context_menu(enabled: bool, trace_label: String, uninstall_label: String) -> CmdResult {
    let app = std::env::current_exe().map_err(err)?;
    shell::set_context_menu(enabled, &app, &trace_label, &uninstall_label).map_err(err)
}

#[tauri::command]
fn open_folder(path: String) -> CmdResult {
    shell::open_folder(&path).map_err(err)
}

#[tauri::command]
fn open_url(url: String) -> CmdResult {
    shell::open_url(&url).map_err(err)
}

#[derive(Deserialize)]
#[serde(tag = "feature", rename_all = "snake_case")]
enum Question {
    Program { id: String },
    Startup { title: String, command: String, exe: String, company: String, source: String, status: String },
    Leftover { app_id: String, target: Target, evidence: String, size_kb: u64, documents: u32 },
}

/// Asks the server's model. What is sent is collected here, from the program's entry and folder.
#[tauri::command(async)]
fn ai_ask(state: State, question: Question, lang: String) -> CmdResult<ai::Answer> {
    let (feature, facts) = match &question {
        Question::Program { id } => ("program", facts::program(&known(&state, std::slice::from_ref(id))?[0])),
        Question::Startup { title, command, exe, company, source, status } => ("startup", facts::startup(title, command, exe, company, source, status)),
        Question::Leftover { app_id, target, evidence, size_kb, documents } => {
            let name = known(&state, std::slice::from_ref(app_id))?.remove(0).name;
            let (item, kind) = match target {
                Target::Dir { path } => (path.clone(), "dir"),
                Target::File { path } => (path.clone(), "file"),
                Target::RegKey { key } => (key.clone(), "registry key"),
                Target::RegValue { key, name } => (format!("{key} -> {name}"), "registry value"),
                Target::Service { name } => (name.clone(), "service"),
            };
            ("leftover", facts::leftover(&name, &item, kind, evidence, *size_kb, *documents))
        }
    };
    ai::ask(rid_core::SERVER, feature, &lang, facts)
}

#[tauri::command]
fn clipboard_text() -> String {
    shell::clipboard_text()
}

#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The newer release `server` offers, if any.
#[tauri::command(async)]
fn check_update() -> CmdResult<Option<update::Release>> {
    update::check(rid_core::SERVER, env!("CARGO_PKG_VERSION"))
}

#[tauri::command]
fn open_registry(key_path: String) -> CmdResult {
    shell::open_registry(&key_path).map_err(err)
}

#[tauri::command]
fn system_tool(name: String) -> CmdResult {
    shell::system_tool(&name).map_err(err)
}

#[tauri::command]
fn load_prefs() -> Value {
    fs::read_to_string(paths::data_dir().join("preferences.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

#[tauri::command]
fn save_prefs(prefs: Value) -> CmdResult {
    let dir = paths::data_dir();
    fs::create_dir_all(&dir).map_err(err)?;
    fs::write(dir.join("preferences.json"), serde_json::to_vec_pretty(&prefs).map_err(err)?).map_err(err)
}

#[tauri::command]
fn save_text_file(path: String, contents: String) -> CmdResult {
    fs::write(path, contents).map_err(err)
}

#[derive(Serialize)]
struct RunResult {
    exit_code: u32,
    cancelled: bool,
    /// The program is still in the installed list: its uninstaller failed, was cancelled, or declined.
    still_installed: bool,
}

#[tauri::command(async)]
fn uninstall_run(state: State, id: String, modify: bool, quiet: bool) -> CmdResult<RunResult> {
    let app = known(&state, std::slice::from_ref(&id))?.remove(0);
    state.cancel.store(false, Ordering::Relaxed);
    let outcome = if modify { uninstall::modify(&app, &state.cancel) } else { uninstall::uninstall(&app, quiet, &state.cancel) }
        .map_err(|e| match e.raw_os_error() {
            Some(740) => "This uninstaller needs administrator rights. Start Rid Uninstaller as administrator.".into(),
            _ => err(e),
        })?;
    let still_installed = inventory::list().apps.iter().any(|a| a.id == id);
    Ok(RunResult { exit_code: outcome.exit_code, cancelled: outcome.cancelled, still_installed })
}

#[tauri::command]
fn uninstall_cancel(state: State) {
    state.cancel.store(true, Ordering::Relaxed);
}

/// For each program, whether it can be uninstalled without any window appearing.
#[tauri::command(async)]
fn quiet_support(state: State, ids: Vec<String>) -> CmdResult<Vec<bool>> {
    Ok(known(&state, &ids)?.iter().map(|a| uninstall::quiet_command(a).is_some()).collect())
}

#[derive(Serialize)]
struct Found {
    app_id: String,
    #[serde(flatten)]
    leftover: Leftover,
}

#[tauri::command(async)]
fn scan_leftovers(state: State, ids: Vec<String>) -> CmdResult<Vec<Found>> {
    let apps = known(&state, &ids)?;
    let installed = inventory::list_full().apps;
    let roots = Roots::from_system();
    let removing: Vec<&str> = ids.iter().map(String::as_str).collect();
    let others = scan::other_program_dirs(&removing, &installed);
    let mut found: Vec<Found> = Vec::new();
    for app in &apps {
        // What the install recording proves comes first; the search by name and location
        // only adds what the recording does not already cover.
        let recorded = traced::load(&app.id).map(|r| scan::traced(&r.items, &roots, &others)).unwrap_or_default();
        let searched = if app.id.starts_with("TRACE:") { Vec::new() } else { scan::scan(app, &installed, &roots) };
        // Package managers keep to their own folders; rules are for ordinary programs.
        let ordinary = !["TRACE:", "SCOOP:", "CHOCO:"].iter().any(|p| app.id.starts_with(p));
        let nominated = if ordinary { rules::leftovers(&rules::fetch(rid_core::SERVER, app), &roots, &others) } else { Vec::new() };
        for leftover in recorded.into_iter().chain(searched).chain(nominated) {
            // Two programs of one batch can point at the same item; it is listed once.
            if !found.iter().any(|f| scan::covered_by(&leftover.target, &f.leftover.target)) {
                found.push(Found { app_id: app.id.clone(), leftover });
            }
        }
    }
    Ok(found)
}

#[tauri::command(async)]
fn processes(state: State, ids: Vec<String>) -> CmdResult<Vec<Proc>> {
    let installed = inventory::list_full().apps;
    let roots = Roots::from_system();
    Ok(known(&state, &ids)?
        .iter()
        .filter_map(|app| scan::program_folder(app, &installed, &roots))
        .flat_map(|dir| procs::running_in(&dir))
        .collect())
}

#[tauri::command(async)]
fn kill_processes(state: State, ids: Vec<String>, pids: Vec<u32>) -> CmdResult<Vec<String>> {
    let installed = inventory::list_full().apps;
    let roots = Roots::from_system();
    let dirs: Vec<_> =
        known(&state, &ids)?.iter().filter_map(|app| scan::program_folder(app, &installed, &roots)).collect();
    // Each pid is re-checked against the programs' folders; a pid from anywhere else is refused.
    Ok(pids
        .iter()
        .filter_map(|&pid| {
            let results: Vec<_> = dirs.iter().map(|dir| procs::kill(pid, dir)).collect();
            if results.iter().any(Result::is_ok) { None } else { Some(format!("{pid}: could not be terminated")) }
        })
        .collect())
}

/// Registry keys that the recordings of `ids` show those programs created.
fn recorded_keys(ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| traced::load(id))
        .flat_map(|r| r.items)
        .filter_map(|item| match item {
            Target::RegKey { key } => Some(key),
            _ => None,
        })
        .collect()
}

fn remove(label: &str, ids: &[String], apps: &[App], targets: &[Target]) -> CmdResult<Report> {
    let installed = inventory::list_full().apps;
    let removing: Vec<&str> = ids.iter().map(String::as_str).collect();
    let others = scan::other_program_dirs(&removing, &installed);
    // Keys this program's recording or its rules name get the wider key check; from the cache
    // only, so that what may be removed is exactly what the scan could have offered.
    let mut keys = recorded_keys(ids);
    keys.extend(apps.iter().flat_map(|a| rules::keys(&rules::cached(a))));
    let report = ops::remove(label, targets, &Roots::from_system(), &others, &keys).map_err(err)?;
    // A recording describes an installation that is now gone. Checked after the removal,
    // which is what takes the program's entry off the list.
    let still_installed = inventory::list().apps;
    for id in ids.iter().filter(|id| !still_installed.iter().any(|a| &a.id == *id)) {
        let _ = traced::forget(id);
    }
    Ok(report)
}

#[tauri::command(async)]
fn remove_leftovers(state: State, label: String, ids: Vec<String>, targets: Vec<Target>, permanent: bool) -> CmdResult<Report> {
    let apps = known(&state, &ids)?;
    let mut report = remove(&label, &ids, &apps, &targets)?;
    // Removal always goes through quarantine, which is where the safety checks and the handling of
    // files in use live; "permanent" empties the batch straight after. Not when files wait for a
    // restart: they arrive in quarantine after it, and without their batch nothing would clear them.
    if permanent
        && report.pending_reboot.is_empty()
        && let Some(batch) = report.batch.take()
        && ops::purge(&batch).is_err()
    {
        report.batch = Some(batch);
    }
    Ok(report)
}

/// Removes only the entries from the installed-programs list, nothing else.
#[tauri::command(async)]
fn remove_entries(state: State, label: String, ids: Vec<String>) -> CmdResult<Report> {
    let targets: Vec<Target> = known(&state, &ids)?
        .iter()
        .filter(|a| !a.reg_path.is_empty())
        .map(|a| Target::RegKey { key: a.reg_path.clone() })
        .collect();
    remove(&label, &ids, &[], &targets)
}

#[tauri::command(async)]
fn quarantine_list() -> Vec<BatchInfo> {
    ops::list()
}

#[tauri::command(async)]
fn quarantine_restore(id: String) -> CmdResult<Vec<String>> {
    ops::restore(&id).map_err(err)
}

#[tauri::command(async)]
fn quarantine_purge(id: String) -> CmdResult {
    ops::purge(&id).map_err(err)
}

/// Drops quarantine batches older than `days`. The retention period is the user's choice,
/// so the window asks for this once its preferences are loaded.
#[tauri::command(async)]
fn quarantine_purge_old(days: u64) -> usize {
    ops::purge_older_than(days)
}

#[tauri::command(async)]
fn startup_list(state: State) -> CmdResult<Vec<startup::Item>> {
    let mut items = startup::list(&Roots::from_system());
    // Store tasks are keyed by package family (`Name_PublisherId`); show the app's display name.
    let apps = state.apps.lock().map_err(err)?;
    for item in items.iter_mut().filter(|i| i.source == startup::Source::StoreApp) {
        let family = item.name.split('\\').next().unwrap_or_default();
        let Some((name, publisher_id)) = family.rsplit_once('_') else { continue };
        let full_name = |a: &App| a.id.strip_prefix("APPX:").map(str::to_owned);
        if let Some(app) = apps.iter().find(|a| {
            full_name(a).is_some_and(|f| f.starts_with(&format!("{name}_")) && f.ends_with(&format!("_{publisher_id}")))
        }) {
            item.title = app.name.clone();
            // A Store task names no program file; the app's logo is shown as its icon instead.
            item.exe = app.icon.clone();
            item.company = app.publisher.clone();
        }
    }
    Ok(items)
}

#[tauri::command(async)]
fn startup_set_enabled(source: startup::Source, name: String, enabled: bool) -> CmdResult {
    startup::set_enabled(source, &name, enabled, &Roots::from_system()).map_err(err)
}

#[tauri::command(async)]
fn startup_delete(source: startup::Source, name: String) -> CmdResult {
    startup::delete(source, &name, &Roots::from_system()).map_err(err)
}

#[tauri::command(async)]
fn startup_add(to: startup::AddTo, name: String, exe: String, arguments: String) -> CmdResult {
    startup::add(to, &name, &exe, &arguments, &Roots::from_system()).map_err(err)
}

/// The UI asks for `icon://localhost/<hex of the UTF-8 icon source>`.
fn icon_response(path: &str) -> Response<Vec<u8>> {
    let bytes = path.trim_start_matches('/').as_bytes();
    let source: Option<String> = bytes
        .chunks(2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
        .collect::<Option<Vec<u8>>>()
        .and_then(|b| String::from_utf8(b).ok());
    let response = match source.as_deref().and_then(icons::png) {
        Some(png) => Response::builder()
            .header("Content-Type", "image/png")
            .header("Cache-Control", "max-age=86400")
            .body(png),
        None => Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()),
    };
    response.expect("static headers are valid")
}

#[derive(Serialize, Deserialize)]
struct WindowPlace {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

fn window_file() -> PathBuf {
    paths::data_dir().join("window.json")
}

fn saved_place() -> Option<WindowPlace> {
    serde_json::from_slice(&fs::read(window_file()).ok()?).ok()
}

/// Puts the window where it was when it was last closed, unless no screen shows that place any more.
fn restore_window(window: &tauri::WebviewWindow) {
    let Some(place) = saved_place() else { return };
    let visible = window.available_monitors().unwrap_or_default().iter().any(|m| {
        let (origin, size) = (m.position(), m.size());
        // Enough of the title bar to grab the window by.
        (origin.x - place.width as i32 + 120..origin.x + size.width as i32 - 120).contains(&place.x)
            && (origin.y - 8..origin.y + size.height as i32 - 60).contains(&place.y)
    });
    if visible && place.width >= 300 && place.height >= 200 {
        let _ = window.set_size(tauri::PhysicalSize::new(place.width, place.height));
        let _ = window.set_position(tauri::PhysicalPosition::new(place.x, place.y));
    }
    if place.maximized {
        let _ = window.maximize();
    }
}

fn save_window(window: &tauri::Window) {
    let maximized = window.is_maximized().unwrap_or(false);
    // A maximized or minimized window reports that state's rectangle; the one to come back to is the last normal one.
    let place = match (maximized || window.is_minimized().unwrap_or(false), saved_place(), window.outer_position(), window.inner_size()) {
        (true, Some(previous), ..) => WindowPlace { maximized, ..previous },
        (false, _, Ok(at), Ok(size)) => WindowPlace { x: at.x, y: at.y, width: size.width, height: size.height, maximized },
        _ => return,
    };
    if let Ok(json) = serde_json::to_vec(&place) {
        let _ = fs::create_dir_all(paths::data_dir()).and_then(|()| fs::write(window_file(), json));
    }
}

fn main() {
    // A few workers, however many icons the list asks for: one thread per request would
    // mean hundreds of threads on a machine with many programs.
    let (icon_jobs, queue) = std::sync::mpsc::channel::<(String, tauri::UriSchemeResponder)>();
    let queue = std::sync::Arc::new(Mutex::new(queue));
    for _ in 0..4 {
        let queue = queue.clone();
        std::thread::spawn(move || {
            loop {
                let job = queue.lock().map(|q| q.recv());
                match job {
                    Ok(Ok((path, responder))) => responder.respond(icon_response(&path)),
                    _ => break,
                }
            }
        });
    }
    inventory::warm_up();
    let mut builder = tauri::Builder::default();
    // Starting Rid again hands the request to the window that is already open. Debug builds
    // skip this, so one can run beside the installed copy while developing.
    if !cfg!(debug_assertions) {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            if let Ok(Some(request)) = launch(args.get(1..).unwrap_or_default(), &app.state::<AppState>()) {
                let _ = app.emit("launch", request);
            }
        }));
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                save_window(window);
            }
        })
        .manage(AppState { prefetch: Mutex::new(Some(std::thread::spawn(inventory::list))), ..AppState::default() })
        .setup(|app| {
            // The window is created hidden and shown here in the theme's own colour, so
            // nothing white is on screen while the page loads.
            if let Some(window) = app.get_webview_window("main") {
                restore_window(&window);
                let dark = match load_prefs().get("theme").and_then(Value::as_str) {
                    Some("dark") => true,
                    Some("light") => false,
                    _ => window.theme().is_ok_and(|t| t == tauri::Theme::Dark),
                };
                let (r, g, b) = if dark { (0x10, 0x11, 0x13) } else { (0xfc, 0xfc, 0xfd) };
                let _ = window.set_background_color(Some(tauri::window::Color(r, g, b, 255)));
                let _ = window.show();
            }
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("icon", move |_ctx, request, responder| {
            let _ = icon_jobs.send((request.uri().path().to_string(), responder));
        })
        .invoke_handler(tauri::generate_handler![
            list_apps,
            msi_locations,
            dir_sizes,
            open_folder,
            open_url,
            open_registry,
            app_version,
            clipboard_text,
            ai_ask,
            check_update,
            system_tool,
            load_prefs,
            save_prefs,
            save_text_file,
            uninstall_run,
            uninstall_cancel,
            quiet_support,
            scan_leftovers,
            processes,
            kill_processes,
            remove_leftovers,
            remove_entries,
            quarantine_list,
            quarantine_restore,
            quarantine_purge,
            quarantine_purge_old,
            startup_list,
            startup_set_enabled,
            startup_delete,
            startup_add,
            driver_status,
            driver_install,
            driver_remove,
            trace_run,
            trace_save,
            trace_revert,
            trace_get,
            trace_forget,
            launch_request,
            context_menu_enabled,
            set_context_menu,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Rid Uninstaller");
}
