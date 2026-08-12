#[cfg(target_os = "windows")]
use std::path::PathBuf;

#[cfg(target_os = "windows")]
const ICON_PATH: &str = "assets/app-icon/app-icon.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");

    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed={ICON_PATH}");
        let icon = manifest_dir().join(ICON_PATH);
        winresource::WindowsResource::new()
            .set_icon(icon.to_str().expect("application icon path must be UTF-8"))
            .compile()
            .expect("failed to embed Windows application icon");
    }
}

#[cfg(target_os = "windows")]
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"))
}
