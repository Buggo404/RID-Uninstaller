//! Which installer built a program or a setup file, and the switches that make it silent.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The installer technology behind a program or a setup file.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Installer {
    #[default]
    Unknown,
    Msi,
    Inno,
    Nsis,
    InstallShield,
    Burn,
    Squirrel,
    SevenZipSfx,
    Msix,
}

const HEAD: u64 = 2 * 1024 * 1024;

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

/// The first two megabytes of a file: enough for the markers, cheap enough to do on demand.
fn head(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(HEAD).read_to_end(&mut bytes);
    }
    bytes
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn sibling(exe: &Path, name: &str) -> bool {
    exe.parent().is_some_and(|dir| dir.join(name).is_file())
}

impl Installer {
    /// `msi` comes from the uninstall entry: an MSI product does not always name MsiExec.
    pub fn detect(exe: &Path, msi: bool) -> Installer {
        if msi || has_extension(exe, "msi") {
            return Installer::Msi;
        }
        if ["msix", "appx", "msixbundle", "appxbundle"]
            .iter()
            .any(|e| has_extension(exe, e))
        {
            return Installer::Msix;
        }
        // Squirrel apps are uninstalled by Update.exe in their own folder, and their setup ships it.
        if exe
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("Update.exe"))
            || sibling(exe, "Update.exe")
        {
            return Installer::Squirrel;
        }
        let bytes = head(exe);
        if contains(&bytes, b"Nullsoft.NSIS") {
            return Installer::Nsis;
        }
        if contains(&bytes, b"Inno Setup Setup Data") || contains(&bytes, b"Inno Setup") {
            return Installer::Inno;
        }
        if contains(&bytes, b"wixburn") || contains(&bytes, b"WixBundle") {
            return Installer::Burn;
        }
        if contains(&bytes, b"InstallShield") && sibling(exe, "_setup.dll") {
            return Installer::InstallShield;
        }
        if contains(&bytes, b";!@Install@!UTF-8!")
            || contains(&bytes, b"7zS.sfx")
            || contains(&bytes, b"7zSD.sfx")
        {
            return Installer::SevenZipSfx;
        }
        Installer::Unknown
    }

    /// `command` with the switches that make this kind of uninstaller run unattended.
    /// `inno_hint`: the uninstall entry ends in `_is1`, which is Inno Setup's own naming.
    /// `None` means the switch cannot be known, and running it interactively is the honest choice.
    pub fn silent_uninstall(self, command: &str, inno_hint: bool) -> Option<String> {
        match self {
            Installer::Inno => Some(format!(
                "{command} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"
            )),
            Installer::Nsis => Some(format!("{command} /S")),
            // Update.exe --uninstall already asks nothing.
            Installer::Squirrel => Some(command.to_string()),
            Installer::Burn => Some(format!("{command} /quiet /norestart")),
            // The rest are docile only when they are really Inno under another name.
            _ if inno_hint => Some(format!(
                "{command} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"
            )),
            // Msi needs the product code, which the caller has; InstallShield's switches differ by
            // version and a wrong one can start a reinstall; self-extractors have no standard switch.
            Installer::Msi
            | Installer::InstallShield
            | Installer::SevenZipSfx
            | Installer::Msix
            | Installer::Unknown => None,
        }
    }

    /// Arguments that make a setup file of this kind run unattended.
    pub fn silent_install(self) -> &'static [&'static str] {
        match self {
            Installer::Inno => &["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"],
            Installer::Nsis => &["/S"],
            Installer::Msi => &["/qn", "/norestart"],
            Installer::Burn => &["/quiet", "/norestart"],
            Installer::InstallShield => &["/s", "/v\"/qn\""],
            Installer::Squirrel | Installer::SevenZipSfx | Installer::Msix | Installer::Unknown => {
                &[]
            }
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Installer::Unknown => "unknown",
            Installer::Msi => "msi",
            Installer::Inno => "inno",
            Installer::Nsis => "nsis",
            Installer::InstallShield => "installshield",
            Installer::Burn => "burn",
            Installer::Squirrel => "squirrel",
            Installer::SevenZipSfx => "sevenzip",
            Installer::Msix => "msix",
        }
    }
}

/// What the details panel shows, and what the trace dialog fills in.
#[derive(Serialize, Debug)]
pub struct Description {
    /// An [`Installer::name`].
    pub kind: &'static str,
    /// The command that uninstalls without a window, when it is known.
    pub silent_uninstall: Option<String>,
    pub silent_install: Vec<String>,
}

/// A setup file's own details. MSI files are recognised by their extension here.
pub fn describe_file(path: &str) -> Description {
    let installer = Installer::detect(Path::new(path), false);
    Description {
        kind: installer.name(),
        silent_uninstall: None,
        silent_install: installer
            .silent_install()
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// A program's installer, read from the executable its uninstall entry names.
pub fn describe(path: &Path, msi: bool, uninstall_string: &str) -> Description {
    let installer = Installer::detect(path, msi);
    let inno_hint = uninstall_string.contains("_is1") || file_stem_is_inno(path);
    let quiet = match installer {
        Installer::Msi => None,
        other => other.silent_uninstall(uninstall_string, inno_hint),
    };
    Description {
        kind: installer.name(),
        silent_uninstall: quiet,
        silent_install: installer
            .silent_install()
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// Inno's uninstaller is always `unins000.exe` (or `uninsNNN.exe`).
fn file_stem_is_inno(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.to_ascii_lowercase().starts_with("unins"))
}

/// The same, for a program whose uninstaller is already known.
pub fn of_program(command: &str, msi: bool) -> Option<Description> {
    let exe: PathBuf = crate::cmdline::exe_of(command)?.into();
    Some(describe(&exe, msi, command))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str, marker: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rid-installer-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut bytes = vec![0u8; 512];
        bytes.extend_from_slice(marker);
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    #[test]
    fn setup_files_are_told_apart_by_their_markers() {
        assert_eq!(
            Installer::detect(&setup("nsis.exe", b"Nullsoft.NSIS3.08"), false),
            Installer::Nsis
        );
        assert_eq!(
            Installer::detect(&setup("inno.exe", b"Inno Setup Setup Data (6.2.0)"), false),
            Installer::Inno
        );
        assert_eq!(
            Installer::detect(&setup("burn.exe", b"wixburn\0\0"), false),
            Installer::Burn
        );
        assert_eq!(
            Installer::detect(&setup("sfx.exe", b";!@Install@!UTF-8!"), false),
            Installer::SevenZipSfx
        );
        assert_eq!(
            Installer::detect(&setup("plain.exe", b"PADDINGPADDING"), false),
            Installer::Unknown
        );
        assert_eq!(
            Installer::detect(&setup("setup.msi", b""), false),
            Installer::Msi
        );
        assert_eq!(
            Installer::detect(&setup("package.msix", b""), false),
            Installer::Msix
        );
        assert_eq!(
            Installer::detect(&setup("app.exe", b""), true),
            Installer::Msi,
            "the uninstall entry says so"
        );
        // InstallShield is only believed when its DLL is next to the setup.
        let lone = setup("ishield.exe", b"InstallShield");
        assert_eq!(Installer::detect(&lone, false), Installer::Unknown);
        std::fs::write(lone.parent().unwrap().join("_setup.dll"), b"").unwrap();
        assert_eq!(Installer::detect(&lone, false), Installer::InstallShield);
    }

    #[test]
    fn silent_switches_are_only_given_where_they_are_known() {
        let cmd = r#""C:\App\unins000.exe""#;
        assert_eq!(
            Installer::Inno.silent_uninstall(cmd, false).unwrap(),
            format!("{cmd} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART")
        );
        assert_eq!(
            Installer::Nsis.silent_uninstall(cmd, false).unwrap(),
            format!("{cmd} /S")
        );
        // Inno Setup's own uninstaller name is proof even when the marker did not survive.
        assert_eq!(
            Installer::Unknown.silent_uninstall(cmd, true).unwrap(),
            format!("{cmd} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART")
        );
        for unknown in [
            Installer::Unknown,
            Installer::InstallShield,
            Installer::SevenZipSfx,
            Installer::Msi,
        ] {
            assert_eq!(
                unknown.silent_uninstall(cmd, false),
                None,
                "{unknown:?} must not be guessed"
            );
        }
        assert_eq!(Installer::Nsis.silent_install(), ["/S"]);
        assert!(Installer::Unknown.silent_install().is_empty());
        assert_eq!(Installer::SevenZipSfx.name(), "sevenzip");
        assert_eq!(describe_file(r"C:\x\setup.msi").kind, "msi");
    }
}
