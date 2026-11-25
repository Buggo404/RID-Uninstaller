//! Newer versions of what is installed, from the package managers and toolchains.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::inventory::{App, Source};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const TIMEOUT: Duration = Duration::from_secs(40);
const USER_AGENT: &str = "RID-Uninstaller (https://github.com/Buggo404/RID-Uninstaller)";

/// One package that has a newer version.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// `<SOURCE>:<package>`: `SCOOP:xmake`, `WINGET:7zip.7zip`, `PIP:requests`.
    pub id: String,
    pub source: Source,
    /// Name shown to the user (the display name when the source has one).
    pub name: String,
    pub installed: String,
    pub available: String,
}

#[derive(Serialize, Default)]
pub struct Updates {
    pub updates: Vec<Update>,
    /// Sources that could not answer, by name; the rest of the list is still usable.
    pub warnings: Vec<String>,
}

/// Package names reach the command line, so only characters packages actually use are allowed.
fn is_package(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-/@".contains(c))
}

/// The prefix of an [`Update::id`], which is how a program's entry and its update find each other.
fn prefix(source: Source) -> &'static str {
    match source {
        Source::Winget => "WINGET",
        Source::Scoop => "SCOOP",
        Source::Chocolatey => "CHOCO",
        Source::Npm => "NPM",
        Source::Pip => "PIP",
        Source::Cargo => "CARGO",
        Source::Dotnet => "DOTNET",
        Source::Vcpkg => "VCPKG",
        Source::Registry | Source::Store | Source::Steam => "?",
    }
}

fn update(source: Source, package: &str, name: &str, installed: &str, available: &str) -> Update {
    Update {
        id: format!("{}:{package}", prefix(source)),
        source,
        name: name.to_string(),
        installed: installed.to_string(),
        available: available.to_string(),
    }
}

/// Runs a program, returning its standard output whatever its exit code: the package tools
/// exit non-zero when they have something to report. `None` when it cannot run or hangs.
fn capture(program: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    command.args(args);
    capture_from(command)
}

/// The same, for the `.cmd` shims of Scoop and npm, which only run through a command processor.
fn capture_shell(line: &str) -> Option<String> {
    let mut command = Command::new(crate::ops::system32("cmd.exe"));
    // `raw_arg` keeps the quotes as written: `cmd /c "` + line + `"` is the one form that both
    // a plain command and a quoted path survive.
    command.arg("/c").raw_arg(format!("\"{line}\""));
    capture_from(command)
}

fn capture_from(mut command: Command) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on another thread: a full pipe would otherwise block a talkative tool forever.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let bytes = reader.join().ok()?;
    // winget, scoop and npm write UTF-8; anything else is only ever shown, never parsed.
    Some(
        String::from_utf8(bytes.clone())
            .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned()),
    )
}

/// The first JSON value in `text`, ignoring notices the tools print around it.
fn first_json(text: &str) -> Option<serde_json::Value> {
    let start = text.find(['{', '['])?;
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()
}

fn http_json(url: &str) -> Option<serde_json::Value> {
    let mut response = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .call()
        .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_string()
        .ok()?;
    serde_json::from_str(&body).ok()
}

fn cargo_exe() -> Option<PathBuf> {
    let cargo = env_dir("USERPROFILE")?
        .join(".cargo")
        .join("bin")
        .join("cargo.exe");
    cargo.is_file().then_some(cargo)
}

/// A path from the environment; variables that exist but are empty count as unset.
fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn scoop_shim() -> Option<PathBuf> {
    let root = env_dir("SCOOP").or_else(|| env_dir("USERPROFILE").map(|p| p.join("scoop")))?;
    let shim = root.join("shims").join("scoop.cmd");
    shim.is_file().then_some(shim)
}

fn choco_exe() -> Option<PathBuf> {
    let root = env_dir("ChocolateyInstall")
        .or_else(|| env_dir("ProgramData").map(|p| p.join("chocolatey")))?;
    let exe = root.join("bin").join("choco.exe");
    exe.is_file().then_some(exe)
}

fn to_path(path: Option<PathBuf>) -> Option<String> {
    path.map(|p| p.to_string_lossy().into_owned())
}

fn scoop() -> Result<Vec<Update>, String> {
    let shim = to_path(scoop_shim()).ok_or("Scoop is not installed")?;
    let text = capture_shell(&format!("\"{shim}\" status")).ok_or("scoop status did not answer")?;
    Ok(parse_scoop(&text))
}

fn parse_scoop(text: &str) -> Vec<Update> {
    let mut out = Vec::new();
    let mut rows = false;
    for line in text.lines() {
        if line.trim_start().starts_with("Name") && line.contains("Version") {
            rows = true; // the table only starts after its heading
            continue;
        }
        if !rows || line.trim_start().starts_with("----") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 || !is_package(parts[0]) || parts[1] == parts[2] {
            continue;
        }
        out.push(update(
            Source::Scoop,
            parts[0],
            parts[0],
            parts[1],
            parts[2],
        ));
    }
    out
}

fn chocolatey() -> Result<Vec<Update>, String> {
    let exe = to_path(choco_exe()).ok_or("Chocolatey is not installed")?;
    let text =
        capture(&exe, &["outdated", "-r", "--no-color"]).ok_or("choco outdated did not answer")?;
    Ok(parse_choco(&text))
}

fn parse_choco(text: &str) -> Vec<Update> {
    let mut out = Vec::new();
    for line in text.lines() {
        // id|current|available|pinned
        let fields: Vec<&str> = line.trim().split('|').collect();
        if fields.len() != 4
            || fields[3].eq_ignore_ascii_case("true")
            || !is_package(fields[0])
            || fields[1] == fields[2]
        {
            continue;
        }
        out.push(update(
            Source::Chocolatey,
            fields[0],
            fields[0],
            fields[1],
            fields[2],
        ));
    }
    out
}

fn npm() -> Result<Vec<Update>, String> {
    let text = capture_shell("npm outdated -g --json").ok_or("npm did not answer")?;
    Ok(parse_npm(&text))
}

fn parse_npm(text: &str) -> Vec<Update> {
    let Some(value) = first_json(text) else {
        return Vec::new();
    };
    let Some(packages) = value.as_object() else {
        return Vec::new();
    };
    packages
        .iter()
        .filter_map(|(name, info)| {
            let available = info["latest"].as_str()?;
            let installed = info["current"]
                .as_str()
                .or_else(|| info["wanted"].as_str())
                .unwrap_or_default();
            is_package(name).then(|| update(Source::Npm, name, name, installed, available))
        })
        .collect()
}

fn pip() -> Result<Vec<Update>, String> {
    let text = capture_shell("pip list --outdated --format=json").ok_or("pip did not answer")?;
    Ok(parse_pip(&text))
}

fn parse_pip(text: &str) -> Vec<Update> {
    let Some(serde_json::Value::Array(items)) = first_json(text) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let name = item["name"].as_str()?;
            let available = item["latest_version"].as_str()?;
            let installed = item["version"].as_str().unwrap_or_default();
            is_package(name).then(|| update(Source::Pip, name, name, installed, available))
        })
        .collect()
}

/// Whether a token is a version, which is what separates a row's data from its name.
fn looks_like_version(token: &str) -> bool {
    token.starts_with(|c: char| c.is_ascii_digit())
        || matches!(token, "未知" | "Unknown" | "—" | "-")
}

fn winget_alias() -> Option<PathBuf> {
    let alias = env_dir("LOCALAPPDATA")?
        .join("Microsoft")
        .join("WindowsApps")
        .join("winget.exe");
    alias.is_file().then_some(alias)
}

fn winget() -> Result<Vec<Update>, String> {
    if winget_alias().is_none() {
        return Ok(Vec::new()); // not installed, so nothing to say
    }
    let text = capture_shell(
        "winget upgrade --include-unknown --accept-source-agreements --disable-interactivity",
    )
    .ok_or("winget did not answer")?;
    Ok(parse_winget(&text))
}

/// Rows are read from their right end: `… name id version available [source]`. Names contain
/// spaces and can overflow into the next column, so neither splitting on spaces nor slicing
/// at the heading's columns gives the same answer for every row.
fn parse_winget(text: &str) -> Vec<Update> {
    let mut out = Vec::new();
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(last) = tokens.last() else { continue };
        let source = usize::from(matches!(*last, "winget" | "msstore"));
        if tokens.len() < 4 + source {
            continue;
        }
        let at = tokens.len() - 3 - source;
        let (id, installed, available) = (tokens[at], tokens[at + 1], tokens[at + 2]);
        if !is_package(id) || id.len() < 3 || at == 0 || id.eq_ignore_ascii_case("id") {
            continue;
        }
        if !looks_like_version(installed)
            || !looks_like_version(available)
            || installed == available
        {
            continue;
        }
        // An unknown installed version is still updatable; an unknown target is not.
        if matches!(available, "未知" | "Unknown") {
            continue;
        }
        out.push(update(
            Source::Winget,
            id,
            &tokens[..at].join(" "),
            installed,
            available,
        ));
    }
    out
}

fn cargo() -> Result<Vec<Update>, String> {
    let Some(exe) = to_path(cargo_exe()) else {
        return Ok(Vec::new());
    };
    let text =
        capture(&exe, &["install", "--list"]).ok_or("cargo install --list did not answer")?;
    Ok(check_crates(&parse_cargo(&text)))
}

/// `agent-browser v0.1.0:` followed by the lines of the binaries it installed.
fn parse_cargo(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|line| !line.starts_with([' ', '\t']))
        .filter_map(|line| {
            let (name, version) = line.trim_end_matches(':').split_once(" v")?;
            is_package(name).then(|| (name.to_string(), version.trim().to_string()))
        })
        .collect()
}

/// crates.io is asked about four crates at a time; a crate it does not know is skipped.
fn check_crates(crates: &[(String, String)]) -> Vec<Update> {
    let mut out = Vec::new();
    std::thread::scope(|scope| {
        for chunk in crates.chunks(4) {
            let found = scope.spawn(move || {
                chunk
                    .iter()
                    .filter_map(|(name, installed)| {
                        let value = http_json(&format!("https://crates.io/api/v1/crates/{name}"))?;
                        let available = value["crate"]["max_stable_version"].as_str()?;
                        (available != installed)
                            .then(|| update(Source::Cargo, name, name, installed, available))
                    })
                    .collect::<Vec<_>>()
            });
            out.extend(found.join().unwrap_or_default());
        }
    });
    out
}

fn dotnet() -> Result<Vec<Update>, String> {
    let Some(exe) = to_path(dotnet_exe()) else {
        return Ok(Vec::new());
    };
    let text = capture(&exe, &["tool", "list", "-g"]).ok_or("dotnet tool list did not answer")?;
    let tools = parse_dotnet(&text);
    let mut out = Vec::new();
    std::thread::scope(|scope| {
        for chunk in tools.chunks(4) {
            let found = scope.spawn(move || {
                chunk
                    .iter()
                    .filter_map(|(id, installed)| {
                        let available = nuget_latest(id)?;
                        (available != *installed)
                            .then(|| update(Source::Dotnet, id, id, installed, &available))
                    })
                    .collect::<Vec<_>>()
            });
            out.extend(found.join().unwrap_or_default());
        }
    });
    Ok(out)
}

/// Rows of `dotnet tool list -g`, which start after the dashed line under its heading.
fn parse_dotnet(text: &str) -> Vec<(String, String)> {
    let mut rows = false;
    text.lines()
        .filter_map(|line| {
            if line.trim_start().starts_with("---") {
                rows = true;
                return None;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            (rows && parts.len() >= 2 && is_package(parts[0]))
                .then(|| (parts[0].to_string(), parts[1].to_string()))
        })
        .collect()
}

/// The newest stable version on nuget.org.
fn nuget_latest(id: &str) -> Option<String> {
    let value = http_json(&format!(
        "https://api.nuget.org/v3-flatcontainer/{}/index.json",
        id.to_lowercase()
    ))?;
    value["versions"]
        .as_array()?
        .iter()
        .rev()
        .filter_map(|v| v.as_str())
        .find(|v| !v.contains('-'))
        .map(str::to_string)
}

fn vcpkg() -> Result<Vec<Update>, String> {
    let Some(exe) = to_path(vcpkg_exe()) else {
        return Ok(Vec::new());
    };
    let text = capture(&exe, &["update"]).ok_or("vcpkg update did not answer")?;
    Ok(parse_vcpkg(&text))
}

/// `fmt:x64-windows 9.1.0 -> 10.2.1` under the "differ from their port versions" heading.
fn parse_vcpkg(text: &str) -> Vec<Update> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some((name, triplet)) = parts.first().and_then(|p| p.split_once(':')) else {
            continue;
        };
        if parts.len() < 4 || parts[2] != "->" || !is_package(name) || parts[1] == parts[3] {
            continue;
        }
        out.push(update(
            Source::Vcpkg,
            &format!("{name}:{triplet}"),
            &format!("{name} ({triplet})"),
            parts[1],
            parts[3],
        ));
    }
    out
}

type Check = fn() -> Result<Vec<Update>, String>;

/// Every source, checked side by side. `on_source` hears about each one as it answers,
/// so the window can show WinGet's list while pip is still thinking.
pub fn list_with(mut on_source: impl FnMut(&str, &Result<Vec<Update>, String>)) -> Updates {
    let sources: [(&str, Check); 8] = [
        ("winget", winget),
        ("scoop", scoop),
        ("chocolatey", chocolatey),
        ("npm", npm),
        ("pip", pip),
        ("cargo", cargo),
        ("dotnet", dotnet),
        ("vcpkg", vcpkg),
    ];
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut results = Updates::default();
    std::thread::scope(|scope| {
        for (name, check) in sources {
            let sender = sender.clone();
            scope.spawn(move || {
                let _ = sender.send((name, check()));
            });
        }
        drop(sender);
        for (name, result) in receiver {
            match &result {
                Ok(found) => results.updates.extend(found.iter().cloned()),
                Err(reason) => results.warnings.push(format!("{name}: {reason}")),
            }
            on_source(name, &result);
        }
    });
    results
        .updates
        .sort_by_cached_key(|u| (u.name.to_lowercase(), u.id.clone()));
    results
}

pub fn list() -> Updates {
    list_with(|_, _| {})
}

/// Marks the programs that have a newer version, for the badge in the program list, and returns
/// which update belongs to which program. Matching is for display only: an update is always
/// installed by its own package manager, from the data the package manager gave.
pub fn attach(apps: &mut [App], updates: &[Update]) -> Vec<(String, Update)> {
    let mut marked = Vec::new();
    for update in updates {
        if let Some(app) = apps.iter_mut().find(|a| a.id == update.id) {
            app.update = Some(update.available.clone());
            marked.push((app.id.clone(), update.clone()));
            continue;
        }
        // Package managers do not share the registry's naming, so a name that is unique in the
        // list is taken as the same program; anything ambiguous is left unmarked.
        let keys = crate::names::name_keys(&update.name);
        if keys.is_empty() {
            continue;
        }
        let mut hits: Vec<&mut App> = apps
            .iter_mut()
            .filter(|a| {
                crate::names::name_keys(&a.name)
                    .iter()
                    .any(|k| keys.contains(k))
            })
            .collect();
        if hits.len() == 1 {
            hits[0].update = Some(update.available.clone());
            marked.push((hits[0].id.clone(), update.clone()));
        }
    }
    marked
}

/// The command that installs the newer version, written for `uninstall::run`.
pub fn upgrade_command(update: &Update) -> Option<String> {
    let package = update.id.split_once(':')?.1;
    if !is_package(package) {
        return None;
    }
    let line = match update.source {
        Source::Winget => format!(
            "winget upgrade --id {package} -e --silent --accept-package-agreements --accept-source-agreements"
        ),
        Source::Scoop => format!("\"{}\" update {package}", to_path(scoop_shim())?),
        Source::Chocolatey => format!(
            "\"{}\" upgrade {package} -y --no-progress",
            to_path(choco_exe())?
        ),
        Source::Npm => format!("npm install -g {package}@latest"),
        Source::Pip => format!("pip install --upgrade {package}"),
        Source::Cargo => format!("\"{}\" install {package} --force", to_path(cargo_exe())?),
        Source::Dotnet => format!("\"{}\" tool update -g {package}", to_path(dotnet_exe())?),
        Source::Vcpkg => format!("\"{}\" upgrade --no-dry-run", to_path(vcpkg_exe())?),
        _ => return None,
    };
    Some(format!(
        "{} /c {line}",
        crate::ops::system32("cmd.exe").display()
    ))
}

fn dotnet_exe() -> Option<PathBuf> {
    let exe = env_dir("ProgramFiles")?.join("dotnet").join("dotnet.exe");
    exe.is_file().then_some(exe)
}

fn vcpkg_exe() -> Option<PathBuf> {
    let exe = env_dir("VCPKG_ROOT")?.join("vcpkg.exe");
    exe.is_file().then_some(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOOP: &str = "WARN  Scoop out of date. Run 'scoop update' to get the latest changes.\n\n\
Name  Installed Version Latest Version Missing Dependencies Info\n\
----  ----------------- -------------- -------------------- ----\n\
xmake 3.0.6             3.0.8                               \n\
zedg  1.0.0             1.1.8                               \n";

    const CHOCO: &str =
        "chocolatey|2.4.3|2.7.5|false\nmaven|3.9.12|3.10.2|false\npinnedtool|1.0|2.0|true\n";

    const NPM: &str = "npm warn Unknown user config \"electron_mirror\".\n{\"agent-browser\":{\"current\":\"0.44.0\",\"wanted\":\"0.44.0\",\"latest\":\"0.45.0\",\"location\":\"C:\\\\npm\"}}\n";

    const PIP: &str = "[{\"name\": \"requests\", \"version\": \"2.33.1\", \"latest_version\": \"2.34.2\", \"latest_filetype\": \"wheel\"}]\n[notice] A new release of pip is available: 26.1.1 -> 26.3\n";

    #[test]
    fn winget_table_is_read_from_its_right_end() {
        // The heading is in Chinese, the way Winget prints it on a Chinese Windows.
        let chinese = "名称                                                         ID                                    版本           可用            源\n\
----------------------------------------------------------------------------------------------------------------------------------------\n\
123云盘                                                      123.123pan                            3.2.7.0        3.3.1.0         winget\n\
7-Zip 26.02 (x64)                                            7zip.7zip                             26.02          26.06           winget\n\
已固定                                                        Pinned.App                            1.0            未知            winget\n\
51 升级可用。\n";
        let found = parse_winget(chinese);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(
            (found[0].id.as_str(), found[0].name.as_str()),
            ("WINGET:123.123pan", "123云盘")
        );
        assert_eq!(
            (found[0].installed.as_str(), found[0].available.as_str()),
            ("3.2.7.0", "3.3.1.0")
        );
        assert_eq!(
            (found[1].id.as_str(), found[1].name.as_str()),
            ("WINGET:7zip.7zip", "7-Zip 26.02 (x64)")
        );

        // Without the source column (`--source winget`) a row has one field fewer.
        let english = "Name                     Id               Version   Available\n\
------------------------------------------------------------\n\
Git                      Git.Git          2.40.0    2.44.0\n";
        let found = parse_winget(english);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            (found[0].id.as_str(), found[0].name.as_str()),
            ("WINGET:Git.Git", "Git")
        );
    }

    /// Reported by a real machine: a long name overflows into the Id column, and an Id as wide
    /// as its column is separated from the version by a single space.
    #[test]
    fn winget_rows_survive_overflowing_names_and_wide_ids() {
        let text = "名称                                     ID                                 版本            可用            源\n\
Microsoft Visual C++ 2012 Redistributable (x64) - 11.0.60610 Microsoft.VCRedist.2012.x64           11.0.60610.1   11.0.61030.0    winget\n\
Visual Studio Community 2022                                 Microsoft.VisualStudio.2022.Community 17.14.33       17.14.45        winget\n";
        let found = parse_winget(text);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].id, "WINGET:Microsoft.VCRedist.2012.x64");
        assert_eq!(
            found[0].name,
            "Microsoft Visual C++ 2012 Redistributable (x64) - 11.0.60610"
        );
        assert_eq!(
            (found[1].id.as_str(), found[1].name.as_str()),
            (
                "WINGET:Microsoft.VisualStudio.2022.Community",
                "Visual Studio Community 2022"
            )
        );
    }

    #[test]
    fn cargo_and_dotnet_lists_are_read() {
        let cargo = "agent-browser v0.44.0:\n    agent-browser.exe\ncargo-edit v0.12.2:\n    cargo-add.exe\n";
        assert_eq!(
            parse_cargo(cargo),
            [
                ("agent-browser".to_string(), "0.44.0".to_string()),
                ("cargo-edit".to_string(), "0.12.2".to_string())
            ]
        );

        let dotnet = "包 ID                    版本              命令             \n\
-------------------------------------------------------\n\
dotnet-dump             9.0.661903      dotnet-dump    \n\
ilspycmd                8.2.0.7535      ilspycmd       \n";
        assert_eq!(parse_dotnet(dotnet).len(), 2);
        assert_eq!(
            parse_dotnet(dotnet)[1],
            ("ilspycmd".to_string(), "8.2.0.7535".to_string())
        );
        assert!(parse_dotnet("没有全局工具").is_empty());
    }

    #[test]
    fn vcpkg_arrows_become_updates() {
        let text = "The following packages are up to date:\n    zlib:x64-windows\n\
The following packages differ from their port versions:\n    fmt:x64-windows 9.1.0 -> 10.2.1\n";
        let found = parse_vcpkg(text);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].id.as_str(), found[0].available.as_str()),
            ("VCPKG:fmt:x64-windows", "10.2.1")
        );
    }

    #[test]
    fn scoop_status_becomes_updates() {
        let found = parse_scoop(SCOOP);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(
            (
                found[0].id.as_str(),
                found[0].installed.as_str(),
                found[0].available.as_str()
            ),
            ("SCOOP:xmake", "3.0.6", "3.0.8")
        );
        assert_eq!(found[1].source, Source::Scoop);
        // The warning above the table is not a package.
        assert!(!found.iter().any(|u| u.name == "WARN"));
    }

    #[test]
    fn chocolatey_rows_ignore_pinned_packages() {
        let found = parse_choco(CHOCO);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, "CHOCO:chocolatey");
        assert!(!found.iter().any(|u| u.name == "pinnedtool"));
    }

    #[test]
    fn npm_json_survives_the_warnings_in_front_of_it() {
        let found = parse_npm(NPM);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (
                found[0].id.as_str(),
                found[0].installed.as_str(),
                found[0].available.as_str()
            ),
            ("NPM:agent-browser", "0.44.0", "0.45.0")
        );
        assert!(parse_npm("{}").is_empty());
    }

    #[test]
    fn pip_json_survives_the_notice_after_it() {
        let found = parse_pip(PIP);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].name.as_str(), found[0].available.as_str()),
            ("requests", "2.34.2")
        );
        assert!(parse_pip("[]").is_empty());
        assert!(parse_pip("pip is not recognised").is_empty());
    }

    #[test]
    fn names_that_could_reach_the_command_line_are_refused() {
        assert!(
            is_package("putty.portable") && is_package("7zip.7zip") && is_package("@scope/pkg")
        );
        assert!(!is_package("a & calc"));
        assert!(!is_package(""));
    }

    #[test]
    fn only_the_matching_program_is_marked() {
        let item = |id: &str, name: &str| App {
            id: id.into(),
            name: name.into(),
            ..App::default()
        };
        // The last two share a name, so neither can be told from the other.
        let mut apps = vec![
            item("SCOOP:xmake", "xmake"),
            item("HKLM:64:7zip", "7-Zip 26.02 (x64)"),
            item("HKLM:64:copy1", "Copy"),
            item("HKLM:64:copy2", "COPY"),
        ];
        let updates = vec![
            Update {
                id: "SCOOP:xmake".into(),
                source: Source::Scoop,
                name: "xmake".into(),
                installed: "3.0.6".into(),
                available: "3.0.8".into(),
            },
            Update {
                id: "WINGET:7zip.7zip".into(),
                source: Source::Winget,
                name: "7-Zip".into(),
                installed: "26.02".into(),
                available: "26.06".into(),
            },
            Update {
                id: "WINGET:copy.copy".into(),
                source: Source::Winget,
                name: "Copy".into(),
                installed: "1".into(),
                available: "2".into(),
            },
        ];
        attach(&mut apps, &updates);
        assert_eq!(
            apps[0].update.as_deref(),
            Some("3.0.8"),
            "the package manager's own entry is matched by id"
        );
        assert_eq!(
            apps[1].update.as_deref(),
            Some("26.06"),
            "a name that is unique in the list is taken as the program"
        );
        assert_eq!(
            (apps[2].update.as_deref(), apps[3].update.as_deref()),
            (None, None),
            "an ambiguous name marks nothing"
        );
    }
}
