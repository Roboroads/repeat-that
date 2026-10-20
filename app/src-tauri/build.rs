use std::path::{Path, PathBuf};

/// The manifest tauri-build embeds by default. It makes Windows load Common Controls v6, which tauri's
/// `common-controls-v6` feature (on by default) imports `TaskDialogIndirect` from.
const WINDOWS_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    ensure_frontend_dist();
    // tauri-build links its manifest into binaries only, so the library's test executable would start without it and
    // fail with STATUS_ENTRYPOINT_NOT_FOUND as soon as it links tauri code that needs Common Controls v6. Leave the
    // manifest out of tauri-build and embed it in every executable ourselves.
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    if let Err(error) = tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)) {
        panic!("tauri-build failed: {error:#}");
    }
    embed_windows_manifest_in_all_executables();
}

/// Links [`WINDOWS_MANIFEST`] into every executable of this package on Windows: the app, its unit tests and any
/// integration tests.
fn embed_windows_manifest_in_all_executables() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let manifest = out_dir.join("app.manifest");
    std::fs::write(&manifest, WINDOWS_MANIFEST).expect("write the manifest");
    // Resource 1 of type 24 (RT_MANIFEST) is where Windows looks for an executable's manifest. The file name differs
    // from tauri-build's `resource.rc` so the two resource files don't overwrite each other.
    let rc = out_dir.join("app-manifest.rc");
    let manifest_path = manifest.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 24 \"{manifest_path}\"\n")).expect("write the resource script");
    embed_resource::compile_for_everything(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("embed the Windows manifest");
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
