fn main() {
    let mut attributes = tauri_build::Attributes::new();
    // Removing programs and tracing installations need administrator rights. Debug builds stay
    // unelevated so that `tauri dev` works from an ordinary terminal; there, protected operations
    // report "access denied".
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("admin.manifest"));
        attributes = attributes.windows_attributes(windows);
    }
    println!("cargo:rerun-if-changed=admin.manifest");
    tauri_build::try_build(attributes).expect("failed to run the Tauri build script");
}