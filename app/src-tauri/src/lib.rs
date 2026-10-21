//! Repeat That desktop app: the audio engine, the tray icon, the hidden settings window and the global hotkeys.

mod actions;
mod autostart;
mod commands;
mod engine;
#[cfg(all(test, target_os = "linux"))]
mod fake_portal;
mod hotkeys;
mod icons;
mod ipc;
mod locking;
#[cfg(test)]
mod mock_runtime_tests;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(all(test, target_os = "linux"))]
mod private_bus;
pub mod self_check;
mod service;
mod session;
mod settings;
mod source;
mod store;
#[cfg(test)]
mod test_app;
mod tray;
#[cfg(target_os = "linux")]
mod webkit;
mod window;

use std::sync::{Arc, Mutex};

use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::actions::SharedEngine;
use crate::engine::{AppEngine, SharedAppEngine};
use crate::icons::{IconCache, SharedIcons};
use crate::tray::SharedStatus;

/// Set by `build.rs` when the frontend was never built and the app embeds a placeholder page instead of it.
const EMBEDS_PLACEHOLDER_FRONTEND: bool = option_env!("REPEAT_THAT_FRONTEND_PLACEHOLDER").is_some();

/// Starts the app. `rt_capture::duck_guard::run_if_requested()` has already run in `main`, so this is never a duck
/// guard process.
pub fn run() {
    // First of all, while the process has only its main thread and before any webview exists: WebKitGTK reads these when
    // the first one starts.
    #[cfg(target_os = "linux")]
    webkit::apply_defaults();
    // `tauri dev` loads the dev server and never shows the embedded page.
    if EMBEDS_PLACEHOLDER_FRONTEND && !cfg!(dev) {
        eprintln!(
            "warning: the frontend was not built, so the settings window shows a placeholder page. Run `npm run build` \
             in app/ and build the app again."
        );
    }
    // Hotkeys, the tray and the window drive the engine; it runs from `setup` on.
    let engine: SharedAppEngine = Arc::new(AppEngine::default());
    let mut builder = tauri::Builder::default()
        // Start order: single instance first, so a second launch exits before it sets anything up, then the plugins the
        // rest of the app uses, then the settings with the hotkeys, the tray and the audio engine (all in `setup`).
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            window::on_second_launch(app, &args)
        }));
    // The plugin's hotkeys need an X server (or Windows, or macOS). On Wayland the portal takes their place, and without
    // a display the plugin could only fail.
    if session::wants_shortcut_plugin(&session::ProcessEnv) {
        builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
    }
    let app = builder
        .plugin(tauri_plugin_store::Builder::new().build())
        // Starting with the OS is off until the user turns it on; the entry starts the app hidden in the tray.
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![window::HIDDEN_ARG]),
        ))
        // Opens the privacy policy in the browser; the capability limits it to this project's pages.
        .plugin(tauri_plugin_opener::init())
        .manage::<SharedStatus>(Mutex::default())
        .manage::<SharedEngine>(engine.clone())
        .manage::<SharedAppEngine>(engine)
        .manage::<SharedIcons>(Arc::new(IconCache::default()))
        .invoke_handler(commands::handler())
        .on_window_event(window::on_window_event)
        .setup(|app| {
            // A tray app: no Dock icon and no app menu bar on macOS. The tray menu and `RunEvent::Reopen` (the app
            // opened again from Finder or Spotlight) are the ways in.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle();
            // What the desktop is like decides how the hotkeys work and whether the window opens at every launch.
            session::install(handle);
            // The settings decide whether this is the first run and which hotkeys to register, so they come first.
            service::install(handle);
            tray::setup(handle)?;
            // Captures the stored target from the start; pausing isn't stored.
            engine::install(handle);
            let args: Vec<String> = std::env::args().collect();
            window::apply_startup_visibility(handle, &args);
            // A launch that asks for an action (`repeat-that --replay`) does it now that the app is up. A second launch
            // never gets here: the running app does the action, see `window::on_second_launch`.
            actions::run_launch_action(&args, handle);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Repeat That app");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { code, api, .. }
            if keep_running_after_exit_request(code, app.webview_windows().len()) =>
        {
            api.prevent_exit();
        }
        // Opening the app again while it runs (Finder, Spotlight, `open`) shows the settings, as a second launch does
        // on the other systems.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => window::show_settings(app),
        // The process ends right after this without dropping the managed state, so the engine is stopped here: that
        // stops a replay, wipes the buffer and gives the captured app its volume back.
        RunEvent::Exit => engine::shutdown(app),
        _ => {}
    });
}

/// Whether to veto an exit request. Only the loss of the last window is vetoed, so the tray keeps the app alive.
/// Everything else goes through: Quit in the tray asks with an exit code, and while the settings window still exists
/// (it only ever hides) a request without a code is not that case, so an OS-driven quit is never blocked. Tauri has
/// already dropped a closed window from its list when it asks about the exit that follows.
fn keep_running_after_exit_request(code: Option<i32>, windows_left: usize) -> bool {
    code.is_none() && windows_left == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn losing_the_last_window_does_not_end_the_app() {
        assert!(keep_running_after_exit_request(None, 0));
    }

    #[test]
    fn quit_from_the_tray_gets_through() {
        assert!(!keep_running_after_exit_request(Some(0), 0));
        assert!(!keep_running_after_exit_request(Some(0), 1));
    }

    #[test]
    fn a_request_while_the_window_exists_gets_through() {
        // The OS asking the app to quit (Cmd+Q, Dock Quit, logoff) must not be vetoed.
        assert!(!keep_running_after_exit_request(None, 1));
    }

    /// The links the settings window opens in the browser.
    fn links_in_the_window() -> Vec<String> {
        let page = std::fs::read_to_string("../src/lib/components/GeneralSection.svelte").expect("the General section");
        page.split(['\'', '"'])
            .filter(|part| part.starts_with("https://"))
            .map(String::from)
            .collect()
    }

    #[test]
    fn the_window_may_open_its_links_and_nothing_else() {
        let capability: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string("capabilities/default.json").unwrap()).unwrap();
        let open_url = capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|permission| permission["identifier"] == "opener:allow-open-url")
            .expect("the window may open URLs");
        let allowed: Vec<&str> = open_url["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["url"].as_str().unwrap())
            .collect();
        assert_eq!(
            allowed,
            ["https://github.com/Roboroads/repeat-that/*"],
            "only this project's pages"
        );
        let links = links_in_the_window();
        assert!(!links.is_empty(), "the privacy policy link");
        for link in links {
            assert!(
                link.starts_with("https://github.com/Roboroads/repeat-that/"),
                "{link} is not allowed"
            );
        }
    }

    /// `tauri.linux.conf.json`, which Tauri merges over `tauri.conf.json` on Linux.
    fn linux_config() -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string("tauri.linux.conf.json").expect("the Linux config")).unwrap()
    }

    #[test]
    fn every_icon_the_linux_package_is_built_from_exists() {
        let config = linux_config();
        let icons = config["bundle"]["icon"].as_array().expect("the icon list");
        let installed = config["bundle"]["linux"]["deb"]["files"]
            .as_object()
            .expect("the files");
        let sources = icons
            .iter()
            .map(|icon| icon.as_str().unwrap())
            .chain(installed.values().map(|source| source.as_str().unwrap()));
        for source in sources {
            assert!(std::path::Path::new(source).is_file(), "{source} is missing");
        }
    }

    #[test]
    fn the_linux_package_installs_the_icon_under_the_name_the_desktop_file_asks_for() {
        // The bundler's desktop file says `Icon=repeat-that` and puts the PNGs of `bundle.icon` under that name in the
        // theme. A desktop that can't find them there (an icon cache that was never rebuilt, a theme that wants
        // another size) is what the extra files are for: a scalable one, a 256 px one and the one in `pixmaps`, which
        // is searched whatever the theme says. They must carry the same name.
        let name = "repeat-that";
        let config = linux_config();
        let installed = config["bundle"]["linux"]["deb"]["files"]
            .as_object()
            .expect("the files");
        let destinations: Vec<&str> = installed.keys().map(String::as_str).collect();
        for wanted in [
            format!("/usr/share/icons/hicolor/scalable/apps/{name}.svg"),
            format!("/usr/share/icons/hicolor/256x256/apps/{name}.png"),
            format!("/usr/share/pixmaps/{name}.png"),
        ] {
            assert!(
                destinations.contains(&wanted.as_str()),
                "{wanted} is not installed: {destinations:?}"
            );
        }
        for destination in destinations {
            let file = destination.rsplit('/').next().unwrap();
            assert_eq!(file.split('.').next(), Some(name), "{destination}");
        }
        // The binary the desktop file starts has the same name.
        let cargo = std::fs::read_to_string("Cargo.toml").unwrap();
        assert!(cargo.contains("name = \"repeat-that\""));
    }

    #[test]
    fn no_icon_is_installed_in_a_directory_that_does_not_match_its_size() {
        // `128x128@2x.png` is 256 px, which the bundler files under `256x256@2`: the directory for a 512 px image.
        let config = linux_config();
        let icons: Vec<&str> = config["bundle"]["icon"]
            .as_array()
            .unwrap()
            .iter()
            .map(|icon| icon.as_str().unwrap())
            .collect();
        assert!(!icons.iter().any(|icon| icon.contains("@2x")), "{icons:?}");
    }

    #[test]
    fn the_placeholder_flag_matches_the_embedded_frontend() {
        // `build.rs` sets the flag exactly when `../dist/index.html` is its placeholder page.
        let index = std::fs::read_to_string("../dist/index.html").expect("../dist/index.html exists after a build");
        assert_eq!(
            EMBEDS_PLACEHOLDER_FRONTEND,
            index.contains("repeat-that: frontend placeholder")
        );
    }
}
