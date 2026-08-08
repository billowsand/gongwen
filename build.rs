#[cfg(target_os = "windows")]
fn main() {
    use std::path::PathBuf;

    const ICON_PATH: &str = "assets/app-icon/app-icon.ico";
    println!("cargo:rerun-if-changed={ICON_PATH}");

    let icon = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join(ICON_PATH);
    winresource::WindowsResource::new()
        .set_icon(icon.to_str().expect("application icon path must be UTF-8"))
        .compile()
        .expect("failed to embed Windows application icon");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
