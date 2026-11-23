use std::path::Path;

fn main() {
    let mut attributes = tauri_build::Attributes::new();
    // Removing programs needs administrator rights. Debug builds stay unelevated so that
    // `tauri dev` works from an ordinary terminal; there, protected operations report "access denied".
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("admin.manifest"));
        attributes = attributes.windows_attributes(windows);
    }
    println!("cargo:rerun-if-changed=admin.manifest");

    // RID_DRIVER names the Microsoft-signed RidMonitor.sys. With it the driver is carried inside
    // the program, which is then complete as a single file.
    println!("cargo:rerun-if-env-changed=RID_DRIVER");
    println!("cargo::rustc-check-cfg=cfg(embedded_driver)");
    if let Some(driver) = std::env::var_os("RID_DRIVER").filter(|d| !d.is_empty()) {
        let driver = Path::new(&driver).canonicalize().expect("RID_DRIVER does not name a file");
        println!("cargo:rerun-if-changed={}", driver.display());
        println!("cargo:rustc-env=RID_DRIVER_FILE={}", driver.display());
        println!("cargo:rustc-cfg=embedded_driver");
    }

    tauri_build::try_build(attributes).expect("failed to run the Tauri build script");
}
