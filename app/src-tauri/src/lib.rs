//! Repeat That desktop app: the audio engine, the tray icon, the hidden settings window and the global hotkeys.

mod actions;
mod autostart;
mod commands;
mod engine;
mod hotkeys;
mod icons;
mod ipc;
mod locking;
#[cfg(test)]
mod mock_runtime_tests;
mod service;
mod settings;
mod source;
mod store;
#[cfg(test)]
mod test_app;
mod tray;
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
    // `tauri dev` loads the dev server and never shows the embedded page.
    if EMBEDS_PLACEHOLDER_FRONTEND && !cfg!(dev) {
        eprintln!(
            "warning: the frontend was not built, so the settings window shows a placeholder page. Run `npm run build` \
             in app/ and build the app again."
        );
    }
    // Hotkeys, the tray and the window drive the engine; it runs from `setup` on.
    let engine: SharedAppEngine = Arc::new(AppEngine::default());
    let app = tauri::Builder::default()
        // Start order: single instance first, so a second launch exits before it sets anything up, then the plugins the
        // rest of the app uses, then the settings with the hotkeys, the tray and the audio engine (all in `setup`).
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            window::on_second_launch(app, &args)
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
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
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::update_settings,
            commands::suspend_hotkeys,
            commands::get_status,
            commands::list_apps,
            commands::list_output_devices,
            commands::select_app,
            commands::set_paused,
            commands::replay,
            commands::stop,
            commands::catch_up,
        ])
        .on_window_event(window::on_window_event)
        .setup(|app| {
            // A tray app: no Dock icon and no app menu bar on macOS. The tray menu and `RunEvent::Reopen` (the app
            // opened again from Finder or Spotlight) are the ways in.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle();
            // The settings decide whether this is the first run and which hotkeys to register, so they come first.
            service::install(handle);
            tray::setup(handle)?;
            // Captures the stored target from the start; pausing isn't stored.
            engine::install(handle);
            let args: Vec<String> = std::env::args().collect();
            window::apply_startup_visibility(handle, &args);
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
