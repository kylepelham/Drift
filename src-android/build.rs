fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dist = manifest.join("..").join("dist");

    // The UI is the desktop's own Vite build, served to the WebView by the app's local proxy.
    std::fs::create_dir_all(&dist).expect("failed to prepare frontend dist directory");
    println!("cargo:rerun-if-changed={}", dist.display());

    tauri_build::build();
}
