use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use rid_core::inventory::{self, App};
use rid_core::safety::Roots;
use rid_core::scan::{self, Leftover, Target};
use rid_core::{ops, uninstall};

const USAGE: &str = "usage:
  rid list [--json | --csv]        installed programs
  rid scan <name> [--json]         what removing <name> would delete (changes nothing)
  rid scan <name> <server> --rules   the same, also asking the rules server at <server>
  rid uninstall <name> [options]   run the program's uninstaller, then remove leftovers
  rid force-remove <name> [options]  remove files and registry items without the uninstaller
  rid remove-entry <name> [options]  remove only the entry from the installed-programs list
  rid history                      removals that can still be undone
  rid startup                      programs that start with Windows
  rid trace '<command line>' [--msi]  run a setup program and record what it changes (as administrator)
  rid context-menu on|off          add or remove Rid's entries in Explorer's right-click menu
  rid restore <batch>              undo a removal
  rid purge <batch>                delete a removal from quarantine for good
  rid key <program name>           the key a program's leftover rules are filed under on the server

options:
  --quiet     uninstall: use the installer's silent mode; fails if it has none
  --no-clean  uninstall: do not remove leftovers afterwards
  --all       also remove leftovers Rid is less sure about (shown as [ ] by scan)
  --dry-run   show what would be done and stop
  --yes       do not ask for confirmation

<name> is a program name as shown by `rid list`, or a unique part of it.";

type Failure = String;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (flags, words): (Vec<&str>, Vec<&str>) = args.iter().map(String::as_str).partition(|a| a.starts_with("--"));
    let has = |flag: &str| flags.contains(&flag);
    let known = ["--json", "--csv", "--quiet", "--no-clean", "--all", "--dry-run", "--yes", "--msi", "--rules"];
    if let Some(bad) = flags.iter().find(|f| !known.contains(f)) {
        eprintln!("unknown option {bad}\n\n{USAGE}");
        return ExitCode::from(2);
    }
    let opts = Options { all: has("--all"), dry_run: has("--dry-run"), yes: has("--yes") };
    let result = match words[..] {
        ["list"] => list(has("--json"), has("--csv")),
        ["scan", name] => scan_one(name, has("--json"), None),
        ["scan", name, server] if has("--rules") => scan_one(name, has("--json"), Some(server)),
        ["uninstall", name] => uninstall_one(name, has("--quiet"), !has("--no-clean"), &opts),
        ["force-remove", name] => force_remove(name, &opts),
        ["remove-entry", name] => remove_entry(name, &opts),
        ["history"] => history(),
        ["key", name] => rid_core::names::name_keys(name).pop().map(|key| println!("{key}")).ok_or_else(|| "this name is too generic to file rules under".into()),
        ["trace", command] => trace(command, has("--msi")),
        ["context-menu", switch @ ("on" | "off")] => std::env::current_exe()
            .and_then(|cli| {
                // Explorer starts the window program, which sits beside this one.
                rid_core::shell::set_context_menu(
                    switch == "on",
                    &cli.with_file_name("rid-app.exe"),
                    "Install and trace with Rid Uninstaller",
                    "Uninstall with Rid Uninstaller",
                )
            })
            .map_err(|e| e.to_string()),
        ["startup"] => {
            for i in rid_core::startup::list(&Roots::from_system()) {
                println!("{:<11} {:<15} {:<36.36} {}", format!("{:?}", i.status), format!("{:?}", i.source), i.title, i.command);
            }
            Ok(())
        }
        ["restore", batch] => restore(batch),
        ["purge", batch] => ops::purge(batch).map(|at_restart| at_restart.iter().for_each(|f| println!("in use, deleted at the next restart: {f}"))).map_err(|e| e.to_string()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    all: bool,
    dry_run: bool,
    yes: bool,
}

fn print_json(value: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(value).expect("plain data"));
}

fn list(json: bool, csv: bool) -> Result<(), Failure> {
    let inv = inventory::list();
    if json {
        print_json(&inv);
    } else if csv {
        let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        println!("name,version,publisher,category,installed,size_kb,location");
        for a in &inv.apps {
            println!(
                "{},{},{},{:?},{},{},{}",
                quote(&a.name),
                quote(&a.version),
                quote(&a.publisher),
                a.category,
                a.install_date.as_deref().unwrap_or_default(),
                a.size_kb.map(|kb| kb.to_string()).unwrap_or_default(),
                quote(&a.install_location)
            );
        }
    } else {
        for a in &inv.apps {
            println!(
                "{:<8} {:<60.60} {:<20.20} {}",
                format!("{:?}", a.category).to_lowercase(),
                a.name,
                a.version,
                a.install_date.as_deref().unwrap_or("-")
            );
        }
        eprintln!("{} programs", inv.apps.len());
    }
    for w in &inv.warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
}

/// The one program whose name equals `name`, or failing that the one containing it.
fn find<'a>(apps: &'a [App], name: &str) -> Result<&'a App, Failure> {
    let wanted = name.to_lowercase();
    let exact: Vec<&App> = apps.iter().filter(|a| a.name.to_lowercase() == wanted).collect();
    let hits = if exact.is_empty() { apps.iter().filter(|a| a.name.to_lowercase().contains(&wanted)).collect() } else { exact };
    match hits[..] {
        [one] => Ok(one),
        [] => Err(format!("no installed program matches '{name}'")),
        _ => Err(format!(
            "'{name}' matches {} programs, be more specific:\n{}",
            hits.len(),
            hits.iter().map(|a| format!("  {}", a.name)).collect::<Vec<_>>().join("\n")
        )),
    }
}

fn describe(target: &Target) -> String {
    match target {
        Target::Dir { path } => format!("dir      {path}"),
        Target::File { path } => format!("file     {path}"),
        Target::RegKey { key } => format!("key      {key}"),
        Target::RegValue { key, name } => format!("value    {key} -> {name}"),
        Target::Service { name } => format!("service  {name}"),
    }
}

fn print_leftovers(found: &[Leftover], all: bool) {
    for l in found {
        let mark = if l.certain || all { 'x' } else { ' ' };
        println!("  [{mark}] {}  ({:?}, {} KB)", describe(&l.target), l.evidence, l.size_kb);
    }
}

fn scan_one(name: &str, json: bool, rules_server: Option<&str>) -> Result<(), Failure> {
    let inv = inventory::list_full();
    let app = find(&inv.apps, name)?;
    let roots = Roots::from_system();
    let mut found = scan::scan(app, &inv.apps, &roots);
    if let Some(server) = rules_server {
        let others = scan::other_program_dirs(&[&app.id], &inv.apps);
        let nominated = rid_core::rules::leftovers(&rid_core::rules::fetch(server, app), &roots, &others);
        for leftover in nominated {
            if !found.iter().any(|f| scan::covered_by(&leftover.target, &f.target)) {
                found.push(leftover);
            }
        }
    }
    if json {
        print_json(&found);
    } else {
        println!("{} ({})", app.name, app.id);
        print_leftovers(&found, false);
    }
    Ok(())
}

fn confirmed(question: &str, opts: &Options) -> Result<bool, Failure> {
    if opts.yes {
        return Ok(true);
    }
    print!("{question} [y/N] ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// Removes `targets` and reports every item. Fails if any item could not be removed.
fn remove(app: &App, targets: &[Target]) -> Result<(), Failure> {
    let installed = inventory::list_full().apps;
    let others = scan::other_program_dirs(&[&app.id], &installed);
    let report = ops::remove(&app.name, targets, &Roots::from_system(), &others, &[]).map_err(|e| e.to_string())?;
    for entry in &report.log {
        println!("  {} {}  ({})", if entry.ok { "ok    " } else { "FAILED" }, describe(&entry.target), entry.message);
    }
    for path in &report.pending_reboot {
        println!("  in use, deleted at next restart (cannot be undone): {path}");
    }
    if let Some(batch) = &report.batch {
        println!("undo with: rid restore {batch}");
    }
    match report.log.iter().filter(|l| !l.ok).count() {
        0 => Ok(()),
        n => Err(format!("{n} items could not be removed")),
    }
}

fn clean(app: &App, installed: &[App], opts: &Options) -> Result<(), Failure> {
    let found = scan::scan(app, installed, &Roots::from_system());
    if found.is_empty() {
        println!("no leftovers found");
        return Ok(());
    }
    print_leftovers(&found, opts.all);
    if opts.dry_run {
        return Ok(());
    }
    let targets: Vec<Target> = found.into_iter().filter(|l| l.certain || opts.all).map(|l| l.target).collect();
    if targets.is_empty() || !confirmed(&format!("Remove the {} marked items?", targets.len()), opts)? {
        return Ok(());
    }
    remove(app, &targets)
}

fn uninstall_one(name: &str, quiet: bool, clean_after: bool, opts: &Options) -> Result<(), Failure> {
    let inv = inventory::list_full();
    let app = find(&inv.apps, name)?;
    let command = if quiet {
        uninstall::quiet_command(app).ok_or_else(|| format!("'{}' has no silent uninstall mode; run without --quiet", app.name))?
    } else {
        app.uninstall_string.clone()
    };
    println!("{} ({})", app.name, app.id);
    println!("  runs: {}", if command.is_empty() { "package removal" } else { &command });
    if opts.dry_run {
        return if clean_after { clean(app, &inv.apps, opts) } else { Ok(()) };
    }
    if !confirmed(&format!("Uninstall '{}'?", app.name), opts)? {
        return Ok(());
    }
    let outcome = uninstall::uninstall(app, quiet, &AtomicBool::new(false)).map_err(|e| e.to_string())?;
    let installed = inventory::list_full().apps;
    if installed.iter().any(|a| a.id == app.id) {
        // Its files are still in use by an installed program, so nothing is cleaned.
        return Err(format!("the uninstaller exited with code {} and the program is still installed", outcome.exit_code));
    }
    println!("uninstalled");
    if clean_after { clean(app, &installed, opts) } else { Ok(()) }
}

fn force_remove(name: &str, opts: &Options) -> Result<(), Failure> {
    let inv = inventory::list_full();
    let app = find(&inv.apps, name)?;
    println!("{} ({}) - the program's uninstaller will NOT be run", app.name, app.id);
    clean(app, &inv.apps, opts)
}

fn remove_entry(name: &str, opts: &Options) -> Result<(), Failure> {
    let inv = inventory::list_full();
    let app = find(&inv.apps, name)?;
    if app.reg_path.is_empty() {
        return Err(format!("'{}' has no registry entry", app.name));
    }
    println!("{} ({})\n  key      {}", app.name, app.id, app.reg_path);
    if opts.dry_run || !confirmed("Remove this entry from the list? Files stay.", opts)? {
        return Ok(());
    }
    remove(app, &[Target::RegKey { key: app.reg_path.clone() }])
}

fn history() -> Result<(), Failure> {
    for b in ops::list() {
        println!("{}  {} items, {} KB  {}", b.id, b.items, b.size_kb, b.label);
    }
    Ok(())
}

/// Runs a setup program and prints what Windows reports it changed.
fn trace(command: &str, follow_msiexec: bool) -> Result<(), Failure> {
    let recording = rid_core::trace::record(command, follow_msiexec, &AtomicBool::new(false), |event| {
        println!("{:>6} {}", event.pid, serde_json::to_string(&event.change).expect("plain data"));
    })
    .map_err(|e| e.to_string())?;
    eprintln!("{} events, {} dropped", recording.events.len(), recording.dropped);
    Ok(())
}

fn restore(batch: &str) -> Result<(), Failure> {
    let problems = ops::restore(batch).map_err(|e| e.to_string())?;
    if problems.is_empty() {
        println!("restored");
        return Ok(());
    }
    Err(format!("not everything could be restored (the rest stays in quarantine):\n  {}", problems.join("\n  ")))
}
