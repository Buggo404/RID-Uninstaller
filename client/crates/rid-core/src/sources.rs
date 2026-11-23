//! Programs that are not in the Windows installed-programs list: Steam games and packages
//! installed with Scoop or Chocolatey.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

use crate::inventory::{App, Category, Source, date_from_unix};

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Package names end up on a command line; anything outside this set is not a package name.
fn is_package_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

fn modified_date(path: &Path) -> Option<String> {
    let secs = fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(date_from_unix(secs))
}

/// The `"key"  "value"` pairs of a Valve KeyValues file, in order, nesting ignored.
// shortcut: lines whose value contains an escaped quote are skipped; write a real parser if a game name needs one.
fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            // `"key" <tabs> "value"` splits on quotes into ["", key, tabs, value, ""].
            let parts: Vec<&str> = line.trim().split('"').collect();
            (parts.len() == 5 && parts[2].trim().is_empty()).then(|| (parts[1].to_string(), parts[3].replace("\\\\", "\\")))
        })
        .collect()
}

fn steam_app(manifest: &str, library: &Path, steam_exe: &Path) -> Option<App> {
    let pairs = vdf_pairs(manifest);
    let get = |key: &str| pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str());
    let appid = get("appid").filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))?;
    Some(App {
        id: format!("STEAM:{appid}"),
        name: get("name")?.to_string(),
        source: Source::Steam,
        install_date: get("LastUpdated").and_then(|s| s.parse::<u64>().ok()).filter(|&s| s > 0).map(date_from_unix),
        size_kb: get("SizeOnDisk").and_then(|s| s.parse::<u64>().ok()).filter(|&b| b > 0).map(|b| b.div_ceil(1024)),
        install_location: library.join("steamapps").join("common").join(get("installdir")?).to_string_lossy().into_owned(),
        // Steam shows its own confirmation and removes the game itself.
        uninstall_string: format!("\"{}\" steam://uninstall/{appid}", steam_exe.display()),
        icon: steam_exe.to_string_lossy().into_owned(),
        ..App::default()
    })
}

/// Where Steam keeps the record of an installed game; it disappears when the game is removed.
pub fn steam_manifest(app: &App) -> Option<PathBuf> {
    let appid = app.id.strip_prefix("STEAM:")?;
    let steamapps = Path::new(&app.install_location).parent()?.parent()?;
    Some(steamapps.join(format!("appmanifest_{appid}.acf")))
}

fn steam() -> Vec<App> {
    let root: Option<String> = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\WOW6432Node\Valve\Steam")
        .and_then(|k| k.get_value("InstallPath"))
        .or_else(|_| RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam").and_then(|k| k.get_value("SteamPath")))
        .ok();
    let Some(root) = root.map(PathBuf::from) else { return Vec::new() };
    let exe = root.join("steam.exe");
    let mut libraries: Vec<PathBuf> = fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf"))
        .map(|text| vdf_pairs(&text).into_iter().filter(|(k, _)| k == "path").map(|(_, v)| PathBuf::from(v)).collect())
        .unwrap_or_default();
    if libraries.is_empty() {
        libraries.push(root);
    }
    let mut apps = Vec::new();
    for library in libraries {
        for entry in fs::read_dir(library.join("steamapps")).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("appmanifest_") && name.ends_with(".acf") {
                apps.extend(fs::read_to_string(entry.path()).ok().and_then(|text| steam_app(&text, &library, &exe)));
            }
        }
    }
    apps
}

fn scoop() -> Vec<App> {
    let user = env_dir("SCOOP").or_else(|| env_dir("USERPROFILE").map(|p| p.join("scoop")));
    let global = env_dir("SCOOP_GLOBAL").or_else(|| env_dir("ProgramData").map(|p| p.join("scoop")));
    let Some(shim) = user.as_ref().map(|root| root.join("shims").join("scoop.cmd")).filter(|s| s.is_file()) else {
        return Vec::new();
    };
    let mut apps = Vec::new();
    for (root, flag) in [(user, ""), (global, " --global")] {
        let Some(root) = root else { continue };
        for entry in fs::read_dir(root.join("apps")).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Scoop lists itself among its apps.
            if name == "scoop" || !is_package_name(&name) || !entry.path().is_dir() {
                continue;
            }
            let current = entry.path().join("current");
            let manifest: serde_json::Value =
                fs::read(current.join("manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            apps.push(App {
                id: format!("SCOOP:{name}"),
                version: manifest["version"].as_str().unwrap_or_default().to_string(),
                url: manifest["homepage"].as_str().unwrap_or_default().to_string(),
                category: Category::Packages,
                source: Source::Scoop,
                install_date: modified_date(&current),
                install_location: entry.path().to_string_lossy().into_owned(),
                uninstall_string: format!("\"{}\" uninstall {name}{flag}", shim.display()),
                name,
                ..App::default()
            });
        }
    }
    apps
}

/// Text of the first `<tag>...</tag>` in a .nuspec file.
// shortcut: XML entities are not decoded; a title containing "&amp;" shows it literally.
fn xml_tag<'a>(xml: &'a str, tag: &str) -> &'a str {
    let open = format!("<{tag}>");
    xml.split_once(&open).and_then(|(_, rest)| rest.split_once(&format!("</{tag}>"))).map(|(text, _)| text.trim()).unwrap_or_default()
}

fn chocolatey() -> Vec<App> {
    let Some(root) = env_dir("ChocolateyInstall").or_else(|| env_dir("ProgramData").map(|p| p.join("chocolatey"))) else {
        return Vec::new();
    };
    let exe = root.join("bin").join("choco.exe");
    if !exe.is_file() {
        return Vec::new();
    }
    let mut apps = Vec::new();
    for entry in fs::read_dir(root.join("lib")).into_iter().flatten().flatten() {
        let folder = entry.file_name().to_string_lossy().into_owned();
        let Ok(nuspec) = fs::read_to_string(entry.path().join(format!("{folder}.nuspec"))) else { continue };
        let id = xml_tag(&nuspec, "id");
        if !is_package_name(id) {
            continue;
        }
        let title = xml_tag(&nuspec, "title");
        apps.push(App {
            id: format!("CHOCO:{id}"),
            name: if title.is_empty() { id.to_string() } else { title.to_string() },
            version: xml_tag(&nuspec, "version").to_string(),
            publisher: xml_tag(&nuspec, "authors").to_string(),
            url: xml_tag(&nuspec, "projectUrl").to_string(),
            // Helper packages other packages depend on, not programs.
            category: if id.ends_with(".extension") { Category::Hidden } else { Category::Packages },
            source: Source::Chocolatey,
            install_date: modified_date(&entry.path()),
            install_location: entry.path().to_string_lossy().into_owned(),
            uninstall_string: format!("\"{}\" uninstall {id} -y", exe.display()),
            icon: exe.to_string_lossy().into_owned(),
            ..App::default()
        });
    }
    apps
}

/// Everything from the extra sources, each read on its own thread.
pub fn list() -> Vec<App> {
    std::thread::scope(|s| {
        let scoop = s.spawn(scoop);
        let chocolatey = s.spawn(chocolatey);
        let mut apps = steam();
        apps.extend(scoop.join().unwrap_or_default());
        apps.extend(chocolatey.join().unwrap_or_default());
        apps
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"228980\"\n\t\"name\"\t\t\"Steamworks Common Redistributables\"\n\t\"installdir\"\t\t\"Steamworks Shared\"\n\t\"LastUpdated\"\t\t\"1783938553\"\n\t\"SizeOnDisk\"\t\t\"2048\"\n\t\"UserConfig\"\n\t{\n\t}\n}\n";

    #[test]
    fn steam_manifest_becomes_a_program() {
        let app = steam_app(MANIFEST, Path::new(r"D:\Games\SteamLibrary"), Path::new(r"C:\Steam\steam.exe")).unwrap();
        assert_eq!(app.id, "STEAM:228980");
        assert_eq!(app.name, "Steamworks Common Redistributables");
        assert_eq!(app.install_location, r"D:\Games\SteamLibrary\steamapps\common\Steamworks Shared");
        assert_eq!(app.uninstall_string, r#""C:\Steam\steam.exe" steam://uninstall/228980"#);
        assert_eq!((app.size_kb, app.install_date.as_deref()), (Some(2), Some("2026-07-13")));
        assert_eq!(steam_manifest(&app).unwrap(), Path::new(r"D:\Games\SteamLibrary\steamapps\appmanifest_228980.acf"));
    }

    #[test]
    fn manifests_that_could_inject_into_the_command_line_are_ignored() {
        let bad = MANIFEST.replace("228980", "1 & calc");
        assert!(steam_app(&bad, Path::new(r"D:\L"), Path::new(r"C:\Steam\steam.exe")).is_none());
        assert!(!is_package_name("git & calc"));
        assert!(!is_package_name(""));
        assert!(is_package_name("putty.portable") && is_package_name("notepad++"));
    }

    #[test]
    fn library_paths_and_nuspec_fields() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"228980\"\t\t\"0\"\n\t\t}\n\t}\n}";
        let paths: Vec<String> = vdf_pairs(vdf).into_iter().filter(|(k, _)| k == "path").map(|(_, v)| v).collect();
        assert_eq!(paths, [r"D:\Program Files (x86)\Steam"]);
        let nuspec = "<package><metadata>\n <id>putty.portable</id>\n <version>0.83.0</version>\n <title>PuTTY (Portable)</title></metadata></package>";
        assert_eq!((xml_tag(nuspec, "id"), xml_tag(nuspec, "title"), xml_tag(nuspec, "authors")), ("putty.portable", "PuTTY (Portable)", ""));
    }
}
