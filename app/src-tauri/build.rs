use std::path::Path;

fn main() {
    ensure_frontend_dist();
    tauri_build::build();
}

/// `tauri::generate_context!` embeds `app/dist` and fails to compile without it. CI and `tauri build` run
/// `npm run build` first; this stub only keeps `cargo clippy/test --workspace` working on a fresh checkout where the
/// frontend was never built. `dist/` is git-ignored.
fn ensure_frontend_dist() {
    let dist = Path::new("../dist");
    let index = dist.join("index.html");
    println!("cargo:rerun-if-changed={}", index.display());
    if index.exists() {
        return;
    }
    std::fs::create_dir_all(dist).expect("create ../dist");
    std::fs::write(
        index,
        "<!doctype html><title>Repeat that?!</title><p>The frontend is not built. Run <code>npm run build</code> in app/.",
    )
    .expect("write ../dist/index.html");
}
