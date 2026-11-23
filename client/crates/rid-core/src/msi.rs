//! Windows Installer bookkeeping read straight from the registry.

use std::collections::HashMap;
use std::path::PathBuf;

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

const USER_DATA: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData";

/// `{23170F69-40C1-2702-0938-000001000000}` -> `96F071321C0420729083000010000000`, the form
/// Windows Installer uses for its own registry keys.
pub fn packed_product_code(guid: &str) -> Option<String> {
    let hex: Vec<char> = guid.trim_matches(['{', '}']).chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.iter().all(char::is_ascii_hexdigit) {
        return None;
    }
    let reversed = |range: std::ops::Range<usize>| hex[range].iter().rev().collect::<String>();
    let mut out = reversed(0..8) + &reversed(8..12) + &reversed(12..16);
    for pair in hex[16..].chunks(2) {
        out.push(pair[1]);
        out.push(pair[0]);
    }
    Some(out.to_uppercase())
}

/// For every installed product (by packed code): the key files of its components.
/// The component list is shared by all products and runs to tens of thousands of keys,
/// so it is read by several threads at once.
pub fn component_files() -> HashMap<String, Vec<String>> {
    let open = |path: &str| RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY);
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(users) = open(USER_DATA) else { return out };
    for sid in users.enum_keys().flatten() {
        let path = format!(r"{USER_DATA}\{sid}\Components");
        let Ok(components) = open(&path) else { continue };
        let names: Vec<String> = components.enum_keys().flatten().collect();
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        let parts: Vec<Vec<(String, String)>> = std::thread::scope(|s| {
            let handles: Vec<_> = names
                .chunks(names.len().div_ceil(workers).max(1))
                .map(|chunk| {
                    let path = &path;
                    s.spawn(move || {
                        let mut found = Vec::new();
                        let Ok(components) = open(path) else { return found };
                        for component in chunk {
                            let Ok(key) = components.open_subkey(component) else { continue };
                            for (product, value) in key.enum_values().flatten() {
                                // Key paths are files (`C:\...`, or `C?\...` for shared ones) or registry
                                // locations (`02:\SOFTWARE\...`); only the files say where the product lives.
                                let file = value.to_string().replacen('?', ":", 1);
                                let b = file.as_bytes();
                                if b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
                                    found.push((product.to_uppercase(), file));
                                }
                            }
                        }
                        found
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (product, file) in parts.into_iter().flatten() {
            out.entry(product).or_default().push(file);
        }
    }
    out
}
/// The deepest folder that contains every one of `files`.
pub fn common_dir(files: &[String]) -> Option<PathBuf> {
    let mut dirs = files.iter().filter_map(|f| f.rsplit_once('\\').map(|(dir, _)| dir));
    let mut common: Vec<&str> = dirs.next()?.split('\\').collect();
    for dir in dirs {
        let same = common.iter().zip(dir.split('\\')).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
        common.truncate(same);
    }
    // At least a drive and one folder; a bare drive is not a program folder.
    (common.len() >= 2).then(|| PathBuf::from(common.join("\\")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_code() {
        assert_eq!(
            packed_product_code("{23170F69-40C1-2702-0938-000001000000}").as_deref(),
            Some("96F071321C0420729083000010000000")
        );
        assert_eq!(packed_product_code("Everything"), None);
        assert_eq!(packed_product_code("{23170F69-40C1-2702-2602}"), None);
    }

    #[test]
    fn common_folder_of_component_files() {
        let files = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            common_dir(&files(&[r"C:\Program Files\7-Zip\7z.exe", r"C:\Program Files\7-Zip\Lang\en.ttt", r"c:\program files\7-ZIP\7z.dll"])),
            Some(PathBuf::from(r"C:\Program Files\7-Zip"))
        );
        assert_eq!(common_dir(&files(&[r"C:\Program Files\App\bin\a.exe"])), Some(PathBuf::from(r"C:\Program Files\App\bin")));
        // Spread over unrelated places: there is no single program folder.
        assert_eq!(common_dir(&files(&[r"C:\Program Files\App\a.exe", r"D:\Data\b.dat"])), None);
        assert_eq!(common_dir(&files(&[r"C:\a.exe", r"C:\Program Files\App\a.exe"])), None);
        assert_eq!(common_dir(&[]), None);
    }
}
