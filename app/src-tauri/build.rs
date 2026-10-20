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

/// Written into the placeholder page so a release build can tell it from the real frontend.
const PLACEHOLDER_MARKER: &str = "<!-- repeat-that: frontend placeholder -->";

/// `tauri::generate_context!` embeds `app/dist` and fails to compile without it. CI and `tauri build` run
/// `npm run build` first. Without a built frontend a debug build gets a placeholder page and a warning, so that
/// `cargo clippy/test --workspace` keeps working on a fresh checkout. A release build stops instead, because it would
/// ship the placeholder as the settings window. `dist/` is ignored by version control, and the next `npm run build`
/// empties it.
fn ensure_frontend_dist() {
    let dist = Path::new("../dist");
    let index = dist.join("index.html");
    // The whole directory, not only the page: `npm run build` empties `dist` first, so a build that fails halfway
    // leaves it without an `index.html` and the script has to run again to notice.
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-changed={}", index.display());

    let page = std::fs::read(&index).ok();
    let is_placeholder = page
        .as_deref()
        .is_some_and(|html| String::from_utf8_lossy(html).contains(PLACEHOLDER_MARKER));
    if page.is_some() && !is_placeholder {
        return;
    }
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        panic!(
            "the frontend is not built, and a release build must not embed the placeholder page. Run `npm run build` \
             in app/ first, or build with `npm run tauri build`, which does that for you."
        );
    }
    if page.is_none() {
        std::fs::create_dir_all(dist).expect("create ../dist");
        let page = format!(
            "{PLACEHOLDER_MARKER}<!doctype html><meta charset=\"utf-8\"><title>FRONTEND NOT BUILT</title>\
             <body style=\"margin:0;padding:2rem;font:16px system-ui,sans-serif;background:#b91c1c;color:#fff\">\
             <h1>FRONTEND NOT BUILT</h1>\
             <p>This is a placeholder page, not the settings window. Run <code>npm run build</code> in \
             <code>app/</code>, then build the app again.</p>"
        );
        std::fs::write(&index, page).expect("write ../dist/index.html");
    }
    println!(
        "cargo:warning=the frontend is not built, so the app embeds a placeholder page (debug builds only). Run `npm run \
         build` in app/ for the real settings window."
    );
    // Lets the app say so at startup (see `run` in lib.rs).
    println!("cargo:rustc-env=REPEAT_THAT_FRONTEND_PLACEHOLDER=1");
}
