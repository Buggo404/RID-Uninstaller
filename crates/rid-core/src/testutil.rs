//! A throwaway folder tree standing in for the machine's well-known folders.

use std::fs;
use std::path::PathBuf;

use crate::safety::Roots;

pub struct Sandbox(pub PathBuf);

impl Sandbox {
    pub fn new(name: &str) -> Sandbox {
        let dir = std::env::temp_dir().join(format!("rid-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Resolved up front so expected paths compare equal to what the code under test resolves.
        let real = fs::canonicalize(&dir).unwrap();
        Sandbox(PathBuf::from(real.to_string_lossy().trim_start_matches(r"\\?\")))
    }

    pub fn dir(&self, rel: &str) -> PathBuf {
        let p = self.0.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    pub fn file(&self, rel: &str) -> PathBuf {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, rel.as_bytes()).unwrap();
        p
    }

    pub fn roots(&self) -> Roots {
        Roots {
            windows: self.dir("Windows"),
            program_files: vec![self.dir("PF"), self.dir("PF86")],
            program_data: self.dir("ProgramData"),
            profile: self.dir("Profile"),
            roaming: self.dir(r"Profile\Roaming"),
            local: self.dir(r"Profile\Local"),
            local_low: self.dir(r"Profile\LocalLow"),
            public: self.dir("Public"),
            desktops: vec![self.dir(r"Profile\Desktop")],
            start_menus: vec![self.dir(r"Profile\Roaming\Microsoft\Windows\Start Menu\Programs")],
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
