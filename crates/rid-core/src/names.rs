//! Decides whether a folder or registry key name refers to a program.
//! Matching is by whole name only: "ToDesk" never matches "Autodesk".

/// Names too common to tell one program from another.
const GENERIC: [&str; 34] = [
    "update", "updater", "tools", "player", "setup", "install", "installer", "uninstall", "common", "config",
    "data", "temp", "cache", "logs", "client", "server", "driver", "drivers", "service", "application", "program",
    "programs", "windows", "microsoft", "system", "user", "users", "default", "desktop", "documents", "downloads",
    "games", "shared", "launcher",
];

const COMPANY_SUFFIXES: [&str; 14] =
    ["inc", "llc", "ltd", "corp", "corporation", "co", "gmbh", "limited", "company", "sa", "ag", "srl", "pbc", "bv"];

/// Comparison form: lower-case letters and digits only.
pub fn key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn is_version(token: &str) -> bool {
    let t = token.trim_start_matches(['v', 'V']);
    t.contains('.') && t.starts_with(|c: char| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// "7-Zip 26.02 (x64)" -> "7-Zip": the display name without version and architecture.
fn without_version(name: &str) -> String {
    let mut depth = 0;
    let flat: String = name
        .chars()
        .filter(|&c| {
            match c {
                '(' | '[' | '（' => depth += 1,
                ')' | ']' | '）' => {
                    depth -= 1;
                    return false;
                }
                _ => {}
            }
            depth == 0
        })
        .collect();
    flat.split_whitespace()
        .filter(|t| {
            let lower = t.to_lowercase();
            !is_version(t) && !matches!(lower.as_str(), "version" | "版本" | "x64" | "x86" | "64-bit" | "32-bit" | "-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn usable(k: &str) -> bool {
    // Short names collide by accident ("Go", "Git" vs unrelated folders).
    k.chars().count() >= 4 && !GENERIC.contains(&k)
}

/// The keys a folder or registry key must equal to count as this program's.
pub fn name_keys(display_name: &str) -> Vec<String> {
    let base = key(&without_version(display_name));
    if !usable(&base) {
        return Vec::new(); // "Tools 1.0" is still just "Tools"
    }
    let mut keys = vec![key(display_name), base];
    keys.dedup();
    keys
}

/// "Adobe Inc." -> "adobe". `None` when the publisher is unknown or too generic to match on.
pub fn publisher_key(publisher: &str) -> Option<String> {
    let words: Vec<String> = publisher
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(key)
        .filter(|w| !w.is_empty() && !COMPANY_SUFFIXES.contains(&w.as_str()))
        .collect();
    let k = words.concat();
    usable(&k).then_some(k)
}

/// Does `entry` (a folder or key name) name the program?
pub fn is_program(entry: &str, keys: &[String]) -> bool {
    keys.contains(&key(entry))
}

/// Same, for an entry found inside the publisher's own folder, where the publisher is
/// often left out of the name: `Mozilla\Firefox` for "Mozilla Firefox".
pub fn is_program_under_publisher(entry: &str, keys: &[String], publisher: &str) -> bool {
    let k = key(entry);
    usable(&k) && (keys.contains(&k) || keys.contains(&format!("{publisher}{k}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_architecture_are_dropped() {
        assert_eq!(name_keys("7-Zip 26.02 (x64)"), ["7zip2602x64", "7zip"]);
        assert_eq!(name_keys("Everything 1.2.1.371"), ["everything121371", "everything"]);
        assert_eq!(name_keys("AULA   F2088Air 版本 1.0.4.1"), ["aulaf2088air版本1041", "aulaf2088air"]);
        assert_eq!(name_keys("Adobe Photoshop 2025"), ["adobephotoshop2025"]);
        assert_eq!(name_keys("Apifox"), ["apifox"]);
    }

    /// Reported against Geek Uninstaller: "Autodesk" offered as a leftover of ToDesk.
    #[test]
    fn similar_names_do_not_match() {
        let todesk = name_keys("ToDesk");
        assert!(is_program("ToDesk", &todesk));
        assert!(is_program("todesk", &todesk));
        assert!(!is_program("Autodesk", &todesk));
        assert!(!is_program("ToDesk_Lite", &todesk));
        let chrome = name_keys("Google Chrome");
        assert!(!is_program("Chrome Remote Desktop", &chrome));
    }

    #[test]
    fn short_and_generic_names_never_match() {
        assert!(name_keys("Go").is_empty());
        assert!(name_keys("Git").is_empty());
        assert!(name_keys("Update").is_empty());
        assert!(name_keys("Tools 1.0").is_empty());
        assert_eq!(publisher_key("Microsoft Corporation"), None);
        assert_eq!(publisher_key(""), None);
    }

    #[test]
    fn publisher_folders() {
        assert_eq!(publisher_key("Adobe Inc.").as_deref(), Some("adobe"));
        assert_eq!(publisher_key("Igor Pavlov").as_deref(), Some("igorpavlov"));
        assert_eq!(publisher_key("NVIDIA Corporation").as_deref(), Some("nvidia"));
        let firefox = name_keys("Mozilla Firefox (x64 en-US)");
        assert!(is_program_under_publisher("Firefox", &firefox, "mozilla"));
        assert!(!is_program_under_publisher("Thunderbird", &firefox, "mozilla"));
        // Reported against Uninstall Tool: the shared NVIDIA\NGX folder offered for an unrelated NVIDIA tool.
        let nsight = name_keys("NVIDIA Nsight Graphics 2024.1");
        assert!(!is_program_under_publisher("NGX", &nsight, "nvidia"));
        assert!(is_program_under_publisher("Nsight Graphics", &nsight, "nvidia"));
    }
}
