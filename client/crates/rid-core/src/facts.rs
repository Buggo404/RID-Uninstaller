//! The facts sent with a question to the model: enough to recognise a program by, nothing personal.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::inventory::App;
use crate::startup::version_string;

const SEP: char = std::path::MAIN_SEPARATOR;

/// Paths under the user's profile carry the account name; it is replaced before anything leaves the machine.
pub fn redact(text: &str) -> String {
    redact_with(text, &std::env::var("USERPROFILE").unwrap_or_default())
}

fn redact_with(text: &str, profile: &str) -> String {
    if profile.len() < 4 {
        return text.to_string();
    }
    let (lower, needle) = (text.to_lowercase(), profile.to_lowercase());
    // Lowercasing can change byte lengths for some scripts; then offsets would not line up.
    if lower.len() != text.len() {
        return text.to_string();
    }
    let mut out = String::new();
    let mut at = 0;
    while let Some(found) = lower[at..].find(&needle) {
        out.push_str(&text[at..at + found]);
        out.push_str("%USERPROFILE%");
        at += found + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

fn size(kb: u64) -> String {
    match kb {
        0 => String::new(),
        kb if kb < 1024 => format!("{kb} KB"),
        kb if kb < 1024 * 1024 => format!("{:.1} MB", kb as f64 / 1024.0),
        kb => format!("{:.2} GB", kb as f64 / 1024.0 / 1024.0),
    }
}

/// What is directly inside `dir`: folders first (marked by a trailing separator), at most 40 names.
fn contents(dir: &Path) -> Vec<String> {
    let mut names: Vec<(bool, String)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(2000)
        .map(|e| (!e.file_type().is_ok_and(|t| t.is_dir()), e.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort_by_key(|(is_file, name)| (*is_file, name.to_lowercase()));
    let more = names.len().saturating_sub(40);
    let mut out: Vec<String> = names.into_iter().take(40).map(|(is_file, name)| if is_file { name } else { format!("{name}{SEP}") }).collect();
    if more > 0 {
        out.push(format!("(and {more} more)"));
    }
    out
}

/// `file.exe: description / product / company` from a program file's version resource.
fn described(exe: &Path) -> Option<String> {
    let path = exe.to_string_lossy();
    let fields: Vec<String> =
        ["FileDescription", "ProductName", "CompanyName"].iter().map(|f| version_string(&path, f)).filter(|v| !v.is_empty()).collect();
    (!fields.is_empty()).then(|| format!("{}: {}", exe.file_name().unwrap_or_default().to_string_lossy(), fields.join(" / ")))
}

/// The largest program files directly in `dir`, with what their version resources say.
fn programs(dir: &Path) -> Vec<String> {
    let mut exes: Vec<(u64, std::path::PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        .filter_map(|e| Some((e.metadata().ok()?.len(), e.path())))
        .collect();
    exes.sort_by_key(|(len, _)| std::cmp::Reverse(*len));
    exes.iter().take(6).filter_map(|(_, path)| described(path)).collect()
}

struct Facts(Map<String, Value>);

impl Facts {
    fn text(&mut self, name: &str, value: &str) {
        let value: String = redact(value.trim()).chars().take(1500).collect();
        if !value.is_empty() {
            self.0.insert(name.into(), Value::String(value));
        }
    }

    fn list(&mut self, name: &str, values: Vec<String>) {
        let values: Vec<Value> = values.iter().take(50).map(|v| Value::String(redact(v).chars().take(250).collect())).collect();
        if !values.is_empty() {
            self.0.insert(name.into(), Value::Array(values));
        }
    }
}

pub fn program(app: &App) -> Map<String, Value> {
    let mut facts = Facts(Map::new());
    facts.text("name", &app.name);
    facts.text("publisher", &app.publisher);
    facts.text("version", &app.version);
    facts.text("installed_on", app.install_date.as_deref().unwrap_or_default());
    facts.text("size", &size(app.size_kb.unwrap_or(0)));
    facts.text("listed_under", &format!("{:?}", app.category));
    facts.text("installed_by", &format!("{:?}", app.source));
    facts.text("website", &app.url);
    facts.text("uninstall_command", &app.uninstall_string);
    facts.text("install_location", &app.install_location);
    if !app.install_location.is_empty() {
        let dir = Path::new(&app.install_location);
        facts.list("folder_contents", contents(dir));
        facts.list("programs", programs(dir));
    }
    facts.0
}

pub fn startup(title: &str, command: &str, exe: &str, company: &str, source: &str, status: &str) -> Map<String, Value> {
    let mut facts = Facts(Map::new());
    facts.text("name", title);
    facts.text("command", command);
    facts.text("starts_from", source);
    facts.text("company", company);
    facts.text("status", status);
    facts.list("program", described(Path::new(exe)).into_iter().collect());
    facts.0
}

/// `item` is the leftover's path or registry location; `kind` and `found_because` are the scan's own words for it.
pub fn leftover(removed_program: &str, item: &str, kind: &str, found_because: &str, size_kb: u64, documents: u32) -> Map<String, Value> {
    let mut facts = Facts(Map::new());
    facts.text("removed_program", removed_program);
    facts.text("item", item);
    facts.text("kind", kind);
    facts.text("found_because", found_because);
    facts.text("size", &size(size_kb));
    if documents > 0 {
        facts.text("document_files_inside", &documents.to_string());
    }
    if kind == "dir" {
        facts.list("folder_contents", contents(Path::new(item)));
    }
    facts.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_name_never_leaves_the_machine() {
        let profile = ["C:", "Users", "Jane Doe"].join(&SEP.to_string());
        let inside = format!("{}{SEP}AppData{SEP}Roaming{SEP}Tool", profile.to_uppercase());
        assert_eq!(redact_with(&inside, &profile), format!("%USERPROFILE%{SEP}AppData{SEP}Roaming{SEP}Tool"));
        let twice = format!("\"{profile}{SEP}a.exe\" --data \"{profile}{SEP}d\"");
        assert!(!redact_with(&twice, &profile).contains("Jane"));
        assert_eq!(redact_with("D:\\Games\\Tool", &profile), "D:\\Games\\Tool");
        assert_eq!(redact_with("anything", ""), "anything");
    }

    #[test]
    fn a_folder_is_described_by_a_bounded_list_of_names() {
        let dir = std::env::temp_dir().join(format!("rid-facts-{}", std::process::id()));
        fs::create_dir_all(dir.join("Sub")).unwrap();
        for i in 0..45 {
            fs::write(dir.join(format!("file{i:02}.txt")), b"x").unwrap();
        }
        let names = contents(&dir);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names.len(), 41);
        assert_eq!(names[0], format!("Sub{SEP}"), "folders come first");
        assert_eq!(names[1], "file00.txt");
        assert_eq!(names[40], "(and 6 more)");
        assert!(contents(Path::new("Z:\\no\\such\\folder")).is_empty());
    }
}
