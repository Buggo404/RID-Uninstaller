//! Leftover rules from the rules server: for one program, places it is known to leave things.
//!
//! A rule only nominates a location. It is expanded against this machine's folders, must
//! exist, must pass the same safety checks as everything else, and is never pre-selected.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::inventory::App;
use crate::safety::{self, Roots};
use crate::scan::{self, Evidence, Leftover, Target};
use crate::{names, paths};

/// How long an answer (including "no rules") is used before the server is asked again.
const FRESH_SECS: u64 = 24 * 3600;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Cached {
    /// 0 when the server has no rules for the program.
    revision: u32,
    rules: Vec<Rule>,
    checked: u64,
}

#[derive(Deserialize)]
struct Answer {
    revision: u32,
    rules: Vec<Rule>,
}

/// The name a program's rules are filed under: its display name without version, letters and digits only.
pub fn key_of(app: &App) -> Option<String> {
    names::name_keys(&app.name).pop()
}

fn cache_file(key: &str) -> io::Result<PathBuf> {
    let dir = paths::secure_dir()?.join("rules");
    fs::create_dir_all(&dir)?;
    let hex: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    Ok(dir.join(format!("{hex}.json")))
}

fn load(key: &str) -> Option<Cached> {
    serde_json::from_slice(&fs::read(cache_file(key).ok()?).ok()?).ok()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Rules decide what is offered for deletion, so they are only taken over an authenticated
/// connection; plain HTTP is accepted for a server on this machine (development).
pub(crate) fn allowed_server(server: &str) -> bool {
    server.starts_with("https://") || server.starts_with("http://127.0.0.1") || server.starts_with("http://localhost")
}

fn ask(server: &str, key: &str, have: u32) -> Result<Option<Answer>, String> {
    let encoded: String = key.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let url = format!("{}/v1/rules/{encoded}", server.trim_end_matches('/'));
    let mut response = ureq::get(&url)
        .header("If-None-Match", format!("\"{have}\""))
        .config()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    match response.status().as_u16() {
        200 => {
            let body = response.body_mut().with_config().limit(1024 * 1024).read_to_string().map_err(|e| e.to_string())?;
            serde_json::from_str(&body).map(Some).map_err(|e| e.to_string())
        }
        304 => Err("unchanged".into()),
        404 => Ok(None),
        other => Err(format!("rules server answered {other}")),
    }
}

/// The rules for `app`, from the cache while it is fresh and from `server` otherwise.
/// Never fails: without an answer the scan simply goes on with what it has.
pub fn fetch(server: &str, app: &App) -> Vec<Rule> {
    let Some(key) = key_of(app) else { return Vec::new() };
    let cached = load(&key).unwrap_or_default();
    if !allowed_server(server) || now().saturating_sub(cached.checked) < FRESH_SECS {
        return cached.rules;
    }
    let updated = match ask(server, &key, cached.revision) {
        Ok(Some(answer)) => Cached { revision: answer.revision, rules: answer.rules, checked: now() },
        Ok(None) => Cached { checked: now(), ..Cached::default() },
        Err(reason) if reason == "unchanged" => Cached { checked: now(), ..cached },
        // Offline or a server problem: keep what we have and try again next time.
        Err(_) => return cached.rules,
    };
    if let (Ok(file), Ok(json)) = (cache_file(&key), serde_json::to_vec(&updated)) {
        let _ = fs::write(file, json);
    }
    updated.rules
}

/// What was last fetched for `app`; never touches the network.
pub fn cached(app: &App) -> Vec<Rule> {
    key_of(app).and_then(|key| load(&key)).map(|c| c.rules).unwrap_or_default()
}

/// `%APPDATA%\Vendor\App` on this machine. `None` for any root not in the fixed list.
fn expand(path: &str, roots: &Roots) -> Option<PathBuf> {
    let (root, rest) = path.split_once('\\')?;
    let base = match root.to_ascii_uppercase().as_str() {
        "%APPDATA%" => &roots.roaming,
        "%LOCALAPPDATA%" => &roots.local,
        "%LOCALAPPDATALOW%" => &roots.local_low,
        "%PROGRAMDATA%" => &roots.program_data,
        "%USERPROFILE%" => &roots.profile,
        "%PROGRAMFILES%" => roots.program_files.first()?,
        "%PROGRAMFILES(X86)%" => roots.program_files.get(1)?,
        _ => return None,
    };
    // The server validates rules too; this side does not rely on that.
    let plain = !rest.is_empty() && rest.split('\\').all(|part| !part.is_empty() && part != "." && part != "..");
    plain.then(|| base.join(rest))
}

/// The rules that name something present on this machine and removable under the safety rules.
pub fn leftovers(rules: &[Rule], roots: &Roots, others: &[PathBuf]) -> Vec<Leftover> {
    rules
        .iter()
        .filter_map(|rule| match rule.kind.as_str() {
            "dir" | "file" => {
                let path = safety::resolve(&expand(&rule.path, roots)?);
                let is_dir = fs::symlink_metadata(&path).ok()?.is_dir();
                safety::check_path(&path, roots, others).ok()?;
                let path = path.to_string_lossy().into_owned();
                Some(if is_dir { Target::Dir { path } } else { Target::File { path } })
            }
            "reg_key" => {
                safety::check_key_traced(&rule.path).ok()?;
                let (hive, below) = rule.path.split_once('\\')?;
                scan::open(hive, below)?;
                Some(Target::RegKey { key: rule.path.clone() })
            }
            _ => None,
        })
        .map(|target| scan::leftover(target, Evidence::Rule, false))
        .collect()
}

/// Registry keys named by `rules`, for the wider key check at removal time.
pub fn keys(rules: &[Rule]) -> Vec<String> {
    rules.iter().filter(|r| r.kind == "reg_key").map(|r| r.path.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    fn rule(kind: &str, path: &str) -> Rule {
        Rule { kind: kind.into(), path: path.into() }
    }

    #[test]
    fn rules_expand_only_from_known_roots() {
        let s = Sandbox::new("rules-expand");
        let roots = s.roots();
        assert_eq!(expand(r"%APPDATA%\Vendor\App", &roots), Some(roots.roaming.join(r"Vendor\App")));
        assert_eq!(expand(r"%programfiles(x86)%\App", &roots), Some(roots.program_files[1].join("App")));
        for bad in [r"%WINDIR%\System32", r"C:\Windows", r"%APPDATA%", r"%APPDATA%\..\x", r"%APPDATA%\a\\b", r"%APPDATA%\"] {
            assert_eq!(expand(bad, &roots), None, "{bad}");
        }
    }

    #[test]
    fn rules_nominate_but_never_preselect_and_cannot_reach_protected_places() {
        let s = Sandbox::new("rules-leftovers");
        let roots = s.roots();
        s.file(r"Profile\Roaming\OddlyNamedData\state.db");
        s.dir(r"Profile\Roaming\Microsoft\Windows");
        let other = s.dir(r"PF\OtherProgram");
        let rules = [
            rule("dir", r"%APPDATA%\OddlyNamedData"),
            rule("dir", r"%APPDATA%\NotOnThisMachine"),
            rule("dir", r"%APPDATA%\Microsoft"),
            rule("dir", r"%PROGRAMFILES%\OtherProgram"),
            rule("dir", r"%APPDATA%\..\..\Profile"),
            rule("reg_key", r"HKLM\SOFTWARE\Microsoft"),
            rule("reg_key", r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip"),
            rule("exec", r"%APPDATA%\OddlyNamedData"),
        ];
        let found = leftovers(&rules, &roots, &[other]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].target, Target::Dir { path: roots.roaming.join("OddlyNamedData").to_string_lossy().into_owned() });
        assert!(!found[0].certain);
        assert_eq!(found[0].evidence, Evidence::Rule);
    }

    #[test]
    fn only_authenticated_or_local_servers_are_asked() {
        assert!(allowed_server("https://rules.example.com"));
        assert!(allowed_server("http://127.0.0.1:18080"));
        assert!(!allowed_server("http://rules.example.com"));
        assert!(!allowed_server("ftp://x"));
        assert!(!allowed_server(""));
    }
}
