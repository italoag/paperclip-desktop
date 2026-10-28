use std::path::Path;

fn main() {
    // `tauri::generate_context!` embeds `frontendDist` into the binary at
    // compile time, but `tauri-build` only emits rerun-if-changed for the config
    // file and bundle resources — not for the frontend itself. Without this,
    // editing `ui/*` changes nothing until some Rust file happens to be touched,
    // and you end up debugging a stale copy of the launcher.
    watch(Path::new("../ui"));

    tauri_build::build()
}

fn watch(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            watch(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
