//! Uninstall commands come from the registry as free text; these pick them apart without a shell.

/// Path of the executable a command line starts with. Handles the unquoted
/// `C:\Program Files\App\unins000.exe /S` form that installers commonly write.
pub fn exe_of(cmd: &str) -> Option<&str> {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        return rest.split('"').next().filter(|p| !p.is_empty());
    }
    let lower = cmd.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(".exe") {
        let end = from + pos + 4;
        // ".exe" must end the path, not sit inside a longer name like "a.exe.config".
        if lower[end..].chars().next().is_none_or(char::is_whitespace) {
            return Some(&cmd[..end]);
        }
        from = end;
    }
    None
}

/// Everything after the executable.
pub fn args_of(cmd: &str) -> &str {
    let cmd = cmd.trim();
    match exe_of(cmd) {
        Some(exe) => {
            let quoted = cmd.starts_with('"');
            cmd[exe.len() + if quoted { 2 } else { 0 }..].trim_start()
        }
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_registry_command_lines() {
        let cases = [
            (r#""C:\Program Files\7-Zip\Uninstall.exe" /S"#, r"C:\Program Files\7-Zip\Uninstall.exe", "/S"),
            (r"C:\Program Files (x86)\App\unins000.exe /SILENT /x", r"C:\Program Files (x86)\App\unins000.exe", "/SILENT /x"),
            (r"D:\Everything\Uninstall.exe", r"D:\Everything\Uninstall.exe", ""),
            ("MsiExec.exe /X{658DA597-F33D-4B4D-8960-0BFCFB80DB73}", "MsiExec.exe", "/X{658DA597-F33D-4B4D-8960-0BFCFB80DB73}"),
            (r"C:\a.exe.config\b.EXE  --x", r"C:\a.exe.config\b.EXE", "--x"),
        ];
        for (cmd, exe, args) in cases {
            assert_eq!(exe_of(cmd), Some(exe), "{cmd}");
            assert_eq!(args_of(cmd), args, "{cmd}");
        }
        assert_eq!(exe_of("rundll32 foo.dll,Bar"), None);
        assert_eq!(exe_of(""), None);
        assert_eq!(exe_of(r#""""#), None);
    }
}
