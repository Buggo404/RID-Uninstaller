//! Installed programs, read from the Uninstall registry keys and the current user's AppX packages.

use serde::Serialize;
use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
use winreg::{HKEY, RegKey};

use crate::safety::{self, Roots};
use crate::{cmdline, msi};
use std::path::Path;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL_WOW: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    #[default]
    Apps,
    System,
    Hidden,
    Store,
    /// Installed by a package manager (Scoop, Chocolatey).
    Packages,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The Windows installed-programs list.
    #[default]
    Registry,
    Store,
    Steam,
    Scoop,
    Chocolatey,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct App {
    /// Stable across runs: `HKLM:64:<key>`, `HKCU:32:<key>`, `APPX:<package full name>`,
    /// `STEAM:<app id>`, `SCOOP:<name>` or `CHOCO:<package id>`.
    pub id: String,
    pub source: Source,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub category: Category,
    /// Id of the program this entry is an update of.
    pub parent: Option<String>,
    /// `YYYY-MM-DD`.
    pub install_date: Option<String>,
    pub size_kb: Option<u64>,
    pub install_location: String,
    pub uninstall_string: String,
    pub quiet_uninstall_string: String,
    pub modify_path: String,
    pub icon: String,
    pub url: String,
    pub reg_path: String,
    pub is_msi: bool,
    pub is_32bit: bool,
    /// An install recording exists for this program. Set by the caller from `traced::list`.
    pub traced: bool,
    /// The entry points at an uninstaller and folder that are both gone: a stale list entry.
    pub broken: bool,
}

#[derive(Serialize, Default)]
pub struct Inventory {
    pub apps: Vec<App>,
    /// Sources that could not be read; the list is still usable without them.
    pub warnings: Vec<String>,
}

pub fn list() -> Inventory {
    let mut inv = Inventory::default();
    let roots: [(HKEY, &str, u32, &str, bool); 4] = [
        (HKEY_LOCAL_MACHINE, "HKLM", KEY_WOW64_64KEY, UNINSTALL, false),
        (HKEY_LOCAL_MACHINE, "HKLM", KEY_WOW64_32KEY, UNINSTALL, true),
        (HKEY_CURRENT_USER, "HKCU", 0, UNINSTALL, false),
        (HKEY_CURRENT_USER, "HKCU", 0, UNINSTALL_WOW, true),
    ];
    for (hive, hive_name, view, path, is_32bit) in roots {
        // A missing root (HKCU WOW6432Node usually is) just means no programs there.
        let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ | view) else { continue };
        for key_name in root.enum_keys().flatten() {
            match root.open_subkey_with_flags(&key_name, KEY_READ | view) {
                Ok(key) => inv.apps.extend(registry_app(&key, hive_name, &key_name, is_32bit)),
                Err(e) => inv.warnings.push(format!("{hive_name}\\{path}\\{key_name}: {e}")),
            }
        }
    }
    match appx_apps() {
        Ok(apps) => inv.apps.extend(apps),
        Err(e) => inv.warnings.push(format!("AppX: {e}")),
    }
    fill_msi_locations(&mut inv.apps);
    let extra = crate::sources::list(&inv.apps);
    inv.apps.extend(extra);
    inv.apps.sort_by_cached_key(|a| a.name.to_lowercase());
    inv
}

/// Many MSI packages do not register an install location. Windows Installer still knows
/// every file it installed for the product, and the folder they share is the answer.
fn fill_msi_locations(apps: &mut [App]) {
    if !apps.iter().any(|a| a.is_msi && a.install_location.is_empty()) {
        return;
    }
    let files = msi::component_files();
    let roots = Roots::from_system();
    let shared: Vec<String> = [&roots.windows]
        .into_iter()
        .cloned()
        .chain(roots.program_files.iter().map(|pf| pf.join("Common Files")))
        .filter_map(|d| safety::norm(&d))
        .collect();
    for app in apps.iter_mut().filter(|a| a.is_msi && a.install_location.is_empty()) {
        let Some(code) = app.id.rsplit(':').next().and_then(msi::packed_product_code) else { continue };
        // Files placed in Windows or Common Files are shared runtime parts, not the program's home.
        let own: Vec<String> = files
            .get(&code)
            .into_iter()
            .flatten()
            .filter(|f| safety::norm(Path::new(f)).is_some_and(|p| !shared.iter().any(|s| safety::within(&p, s))))
            .cloned()
            .collect();
        if let Some(dir) = msi::common_dir(&own).filter(|d| d.is_dir() && safety::check_path(d, &roots, &[]).is_ok()) {
            app.install_location = dir.to_string_lossy().into_owned();
        }
    }
}

fn registry_app(key: &RegKey, hive: &str, key_name: &str, is_32bit: bool) -> Option<App> {
    let text = |name: &str| key.get_value::<String, _>(name).unwrap_or_default().trim().to_string();
    let flag = |name: &str| key.get_value::<u32, _>(name).unwrap_or(0) != 0;

    let name = text("DisplayName");
    if name.is_empty() {
        return None;
    }
    let bits = if is_32bit { "32" } else { "64" };
    let parent_key = text("ParentKeyName");
    let is_msi = flag("WindowsInstaller");
    let (uninstall_string, modify_path) = if is_msi {
        // Installer ignores UninstallString for MSI products; the key name is the product code.
        (format!("MsiExec.exe /X{key_name}"), format!("MsiExec.exe /I{key_name}"))
    } else {
        (text("UninstallString"), text("ModifyPath"))
    };
    let url = [text("URLInfoAbout"), text("HelpLink")].into_iter().find(|u| !u.is_empty()).unwrap_or_default();

    Some(App {
        id: format!("{hive}:{bits}:{key_name}"),
        name,
        version: text("DisplayVersion"),
        publisher: text("Publisher"),
        category: classify(flag("SystemComponent"), &parent_key, &text("ReleaseType")),
        // "OperatingSystem" marks a Windows update, which has no parent entry in the list.
        parent: (!parent_key.is_empty() && parent_key != "OperatingSystem")
            .then(|| format!("{hive}:{bits}:{parent_key}")),
        install_date: parse_install_date(&text("InstallDate")).or_else(|| {
            let t = key.query_info().ok()?.get_last_write_time_system();
            Some(format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay))
        }),
        size_kb: key.get_value::<u32, _>("EstimatedSize").ok().filter(|&kb| kb > 0).map(u64::from),
        install_location: text("InstallLocation").trim_matches('"').to_string(),
        uninstall_string,
        quiet_uninstall_string: text("QuietUninstallString"),
        modify_path: if flag("NoModify") { String::new() } else { modify_path },
        icon: [text("DisplayIcon").trim_matches('"'), cmdline::exe_of(&text("UninstallString")).unwrap_or_default()]
            .into_iter()
            .find(|i| !i.is_empty())
            .unwrap_or_default()
            .to_string(),
        url,
        reg_path: format!(
            "{}\\{}\\{key_name}",
            if hive == "HKLM" { "HKEY_LOCAL_MACHINE" } else { "HKEY_CURRENT_USER" },
            if is_32bit { UNINSTALL_WOW } else { UNINSTALL }
        ),
        is_msi,
        is_32bit,
        source: if key_name.starts_with("Steam App ") { Source::Steam } else { Source::Registry },
        traced: false,
        broken: !is_msi && is_stale(&text("UninstallString"), text("InstallLocation").trim_matches('"')),
    })
}

/// Stale when the uninstaller the entry names is missing and so is its folder (if it names one).
/// An entry that names neither is not stale, just incomplete.
fn is_stale(uninstall_string: &str, install_location: &str) -> bool {
    let Some(uninstaller) = cmdline::exe_of(uninstall_string).map(Path::new) else { return false };
    // Bare names like "MsiExec.exe" are found through the search path, not at a fixed place.
    uninstaller.is_absolute() && !uninstaller.exists() && (install_location.is_empty() || !Path::new(install_location).exists())
}

// shortcut: standard Add/Remove Programs semantics, replace with the original's rules once docs/reverse-notes.md has them.
fn classify(system_component: bool, parent_key: &str, release_type: &str) -> Category {
    let release = release_type.to_ascii_lowercase();
    if system_component {
        Category::Hidden
    } else if !parent_key.is_empty() || release.contains("update") || release.contains("hotfix") {
        Category::System
    } else {
        Category::Apps
    }
}

/// Accepts the documented `YYYYMMDD` form only; installers also write garbage here.
fn parse_install_date(raw: &str) -> Option<String> {
    if raw.len() != 8 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, m, d): (u32, u32, u32) = (raw[..4].parse().ok()?, raw[4..6].parse().ok()?, raw[6..].parse().ok()?);
    ((1980..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d))
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

fn appx_apps() -> windows::core::Result<Vec<App>> {
    let mut apps = Vec::new();
    for pkg in PackageManager::new()?.FindPackagesByUserSecurityId(&HSTRING::new())? {
        // One broken package must not hide the rest.
        if let Ok(Some(app)) = appx_app(&pkg) {
            apps.push(app);
        }
    }
    Ok(apps)
}

fn appx_app(pkg: &Package) -> windows::core::Result<Option<App>> {
    // Frameworks and resource packs are dependencies; system-signed packages cannot be removed.
    if pkg.IsFramework()? || pkg.IsResourcePackage()? || pkg.SignatureKind()? == PackageSignatureKind::System {
        return Ok(None);
    }
    let id = pkg.Id()?;
    let full_name = id.FullName()?.to_string();
    let v = id.Version()?;
    let text = |r: windows::core::Result<HSTRING>| r.map(|s| s.to_string()).unwrap_or_default();
    let display = text(pkg.DisplayName());
    Ok(Some(App {
        id: format!("APPX:{full_name}"),
        name: if display.is_empty() { id.Name()?.to_string() } else { display },
        version: format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision),
        publisher: text(pkg.PublisherDisplayName()),
        category: Category::Store,
        parent: None,
        install_date: pkg.InstalledDate().ok().map(|t| date_from_filetime_ticks(t.UniversalTime)),
        size_kb: None,
        install_location: text(pkg.InstalledPath()),
        uninstall_string: String::new(),
        quiet_uninstall_string: String::new(),
        modify_path: String::new(),
        icon: text(pkg.Logo().and_then(|u| u.RawUri())),
        url: String::new(),
        reg_path: String::new(),
        is_msi: false,
        is_32bit: false,
        source: Source::Store,
        traced: false,
        broken: false,
    }))
}

/// Seconds since 1970-01-01 (UTC) to `YYYY-MM-DD`.
pub(crate) fn date_from_unix(secs: u64) -> String {
    date_from_filetime_ticks(secs as i64 * 10_000_000 + 116_444_736_000_000_000)
}

/// 100 ns ticks since 1601-01-01 (UTC) to `YYYY-MM-DD`.
fn date_from_filetime_ticks(ticks: i64) -> String {
    // Days from 1601-01-01 to 1970-01-01, then the standard civil-from-days conversion.
    let z = ticks.div_euclid(864_000_000_000) - 134_774 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_date_rejects_garbage() {
        assert_eq!(parse_install_date("20240131").as_deref(), Some("2024-01-31"));
        for bad in ["", "2024-01-31", "20241301", "20240100", "00000000", "1/31/2024"] {
            assert_eq!(parse_install_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn filetime_ticks_to_date() {
        assert_eq!(date_from_filetime_ticks(0), "1601-01-01");
        assert_eq!(date_from_filetime_ticks(116_444_736_000_000_000), "1970-01-01");
        // 2024-02-29T12:00:00Z
        assert_eq!(date_from_filetime_ticks(133_536_816_000_000_000), "2024-02-29");
    }

    #[test]
    fn stale_entries() {
        let cmd = r"C:\Windows\System32\cmd.exe";
        assert!(is_stale(r#""C:\RidNoSuchFolder\unins000.exe" /S"#, ""));
        assert!(is_stale(r"C:\RidNoSuchFolder\unins000.exe", r"C:\RidNoSuchFolder"));
        assert!(!is_stale(r"C:\RidNoSuchFolder\unins000.exe", r"C:\Windows"), "the folder is still there");
        assert!(!is_stale(cmd, ""), "the uninstaller exists");
        assert!(!is_stale("MsiExec.exe /X{GUID}", ""), "found through the search path");
        assert!(!is_stale("", ""), "names nothing");
    }

    #[test]
    fn classification() {
        assert_eq!(classify(false, "", ""), Category::Apps);
        assert_eq!(classify(true, "", ""), Category::Hidden);
        assert_eq!(classify(true, "Parent", "Update"), Category::Hidden);
        assert_eq!(classify(false, "Parent", ""), Category::System);
        assert_eq!(classify(false, "", "Security Update"), Category::System);
    }
}
