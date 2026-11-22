#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use rid_core::inventory::{self, App, Inventory};
use rid_core::ops::{self, BatchInfo, Report};
use rid_core::procs::{self, Proc};
use rid_core::safety::Roots;
use rid_core::scan::{self, Leftover, Target};
use rid_core::{driver, icons, paths, shell, startup, trace, traced, uninstall};
use serde::Serialize;
use serde_json::Value;
use tauri::Emitter;
use tauri::http::{Response, StatusCode};

/// Removed items stay restorable for this long.
const QUARANTINE_DAYS: u64 = 30;

type CmdResult<T = ()> = Result<T, String>;

fn err(e: impl ToString) -> String {
    e.to_string()
}

#[derive(Default)]
struct AppState {
    /// The list as last shown. Removal works from this snapshot, because a program that has
    /// just been uninstalled is no longer in the registry but its leftovers still need its details.
    apps: Mutex<Vec<App>>,
    cancel: AtomicBool,
    /// Result of the last install recording, until the user decides what to do with it.
    pending_trace: Mutex<Vec<Target>>,
}

type State<'a> = tauri::State<'a, AppState>;

fn known(state: &AppState, ids: &[String]) -> CmdResult<Vec<App>> {
    let apps = state.apps.lock().map_err(err)?;
    ids.iter()
        .map(|id| apps.iter().find(|a| &a.id == id).cloned().ok_or_else(|| format!("unknown program: {id}")))
        .collect()
}

#[tauri::command(async)]
fn list_apps(state: State) -> CmdResult<Inventory> {
    let mut inv = inventory::list();
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

#[tauri::command(async)]
fn driver_status() -> driver::Status {
    driver::status()
}

/// Installs the driver shipped beside the executable.
#[tauri::command(async)]
fn driver_install() -> CmdResult {
    let image = std::env::current_exe().map_err(err)?.with_file_name("RidMonitor.sys");
    driver::install(&image).map_err(err)
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
    driver::load().map_err(|e| format!("The tracing driver is not available: {e}"))?;
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
    let others = scan::other_program_dirs(&[], &inventory::list().apps);
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

#[derive(Serialize)]
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

/// What the command line asked for (Explorer's context menu starts Rid this way).
#[tauri::command(async)]
fn launch_request(state: State) -> CmdResult<Option<Launch>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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

#[tauri::command(async)]
fn dir_size(path: String) -> u64 {
    paths::dir_size_kb(Path::new(&path))
}

#[tauri::command]
fn open_folder(path: String) -> CmdResult {
    shell::open_folder(&path).map_err(err)
}

#[tauri::command]
fn open_url(url: String) -> CmdResult {
    shell::open_url(&url).map_err(err)
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
    let installed = inventory::list().apps;
    let roots = Roots::from_system();
    let removing: Vec<&str> = ids.iter().map(String::as_str).collect();
    let others = scan::other_program_dirs(&removing, &installed);
    let mut found: Vec<Found> = Vec::new();
    for app in &apps {
        // What the install recording proves comes first; the search by name and location
        // only adds what the recording does not already cover.
        let recorded = traced::load(&app.id).map(|r| scan::traced(&r.items, &roots, &others)).unwrap_or_default();
        let searched = if app.id.starts_with("TRACE:") { Vec::new() } else { scan::scan(app, &installed, &roots) };
        for leftover in recorded.into_iter().chain(searched) {
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
    let installed = inventory::list().apps;
    let roots = Roots::from_system();
    Ok(known(&state, &ids)?
        .iter()
        .filter_map(|app| scan::program_folder(app, &installed, &roots))
        .flat_map(|dir| procs::running_in(&dir))
        .collect())
}

#[tauri::command(async)]
fn kill_processes(state: State, ids: Vec<String>, pids: Vec<u32>) -> CmdResult<Vec<String>> {
    let installed = inventory::list().apps;
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

fn remove(label: &str, ids: &[String], targets: &[Target]) -> CmdResult<Report> {
    let installed = inventory::list().apps;
    let removing: Vec<&str> = ids.iter().map(String::as_str).collect();
    let others = scan::other_program_dirs(&removing, &installed);
    let report = ops::remove(label, targets, &Roots::from_system(), &others, &recorded_keys(ids)).map_err(err)?;
    // A recording describes an installation that is now gone. Checked after the removal,
    // which is what takes the program's entry off the list.
    let still_installed = inventory::list().apps;
    for id in ids.iter().filter(|id| !still_installed.iter().any(|a| &a.id == *id)) {
        let _ = traced::forget(id);
    }
    Ok(report)
}

#[tauri::command(async)]
fn remove_leftovers(state: State, label: String, ids: Vec<String>, targets: Vec<Target>) -> CmdResult<Report> {
    known(&state, &ids)?;
    remove(&label, &ids, &targets)
}

/// Removes only the entries from the installed-programs list, nothing else.
#[tauri::command(async)]
fn remove_entries(state: State, label: String, ids: Vec<String>) -> CmdResult<Report> {
    let targets: Vec<Target> = known(&state, &ids)?
        .iter()
        .filter(|a| !a.reg_path.is_empty())
        .map(|a| Target::RegKey { key: a.reg_path.clone() })
        .collect();
    remove(&label, &ids, &targets)
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

fn main() {
    std::thread::spawn(|| ops::purge_older_than(QUARANTINE_DAYS));
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .register_asynchronous_uri_scheme_protocol("icon", move |_ctx, request, responder| {
            let _ = icon_jobs.send((request.uri().path().to_string(), responder));
        })
        .invoke_handler(tauri::generate_handler![
            list_apps,
            dir_size,
            open_folder,
            open_url,
            open_registry,
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
