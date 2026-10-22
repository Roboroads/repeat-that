//! Repeat That desktop app: the audio engine, the tray icon, the hidden settings window and the global hotkeys.

mod actions;
mod autostart;
mod commands;
mod engine;
#[cfg(all(test, target_os = "linux"))]
mod fake_portal;
pub mod host_env;
mod hotkeys;
mod icons;
mod ipc;
#[cfg(target_os = "linux")]
mod kglobalaccel;
#[cfg(test)]
mod linux_package_tests;
mod locking;
#[cfg(test)]
mod mock_runtime_tests;
mod permission;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(all(test, target_os = "linux"))]
mod private_bus;
#[cfg(target_os = "linux")]
mod sandbox;
pub mod self_check;
mod service;
mod session;
mod settings;
mod shortcut_page;
mod source;
mod store;
#[cfg(test)]
mod test_app;
mod tray;
mod updater;
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
    // Only a build that has the update key compiled in has the updater; without one nothing is registered, the window
    // gets no way to update, and the app never asks the network about updates.
    if let Some(updater) = updater::plugin() {
        builder = builder.plugin(updater);
    }
    // The plugin reads its key and where to look from the app's config, which is made here instead of in
    // `tauri.conf.json` so that a build without a key has none of it.
    let mut context = tauri::generate_context!();
    updater::configure(&mut context);
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
        .manage::<shortcut_page::PageOpener>(shortcut_page::real_opener())
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
            // Before the tray, which starts with an update item where the app looks for updates.
            updater::install(handle);
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
        .build(context)
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
        RunEvent::Exit => {
            updater::shutdown(app);
            engine::shutdown(app);
        }
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

    /// The links the settings window opens in the browser: the privacy policy and the macOS issue form.
    fn links_in_the_window() -> Vec<String> {
        ["GeneralSection.svelte", "MacosNotice.svelte"]
            .into_iter()
            .flat_map(|file| {
                let page = std::fs::read_to_string(format!("../src/lib/components/{file}")).expect(file);
                page.split(['\'', '"'])
                    .filter(|part| part.starts_with("https://"))
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn the_window_may_open_its_links_and_nothing_else() {
        // Links go through the app's `open_link` command, which opens this project's pages only (and starts the
        // browser with the host's environment from an AppImage). The window gets no opener permission of its own, so
        // it can't go around that.
        let capability: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string("capabilities/default.json").unwrap()).unwrap();
        let permissions = capability["permissions"].as_array().unwrap();
        assert!(
            !permissions.iter().any(|permission| {
                let id = permission
                    .as_str()
                    .or_else(|| permission["identifier"].as_str())
                    .unwrap_or_default();
                id.starts_with("opener:")
            }),
            "{permissions:?}"
        );
        // Nor may it use the updater plugin itself: it could point a download anywhere the key allows. It only calls the
        // app's own update commands, which fetch from one fixed place.
        assert!(
            !permissions.iter().any(|permission| {
                let id = permission
                    .as_str()
                    .or_else(|| permission["identifier"].as_str())
                    .unwrap_or_default();
                id.starts_with("updater:")
            }),
            "{permissions:?}"
        );
        assert_eq!(commands::LINKS, "https://github.com/Roboroads/repeat-that/");
        let links = links_in_the_window();
        assert!(
            links.len() >= 2,
            "the privacy policy and the macOS issue form: {links:?}"
        );
        for link in links {
            assert!(commands::is_project_link(&link), "{link} is not allowed");
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
