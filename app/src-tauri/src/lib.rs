//! Repeat that?! desktop shell: tray icon, hidden settings window and the plugins the rest of the app builds on.

mod actions;
mod autostart;
mod commands;
mod hotkeys;
mod ipc;
#[cfg(test)]
mod mock_runtime_tests;
mod service;
mod settings;
mod tray;
mod window;

use std::sync::{Arc, Mutex};

use tauri::RunEvent;
use tauri_plugin_autostart::MacosLauncher;

use crate::actions::{NoEngine, SharedEngine};
use crate::tray::{SharedStatus, TrayStatus};

/// Starts the app. `rt_capture::duck_guard::run_if_requested()` has already run in `main`, so this is never a duck
/// guard process.
pub fn run() {
    let app = tauri::Builder::default()
        // Start order: single instance first, so a second launch exits before it sets anything up, then the plugins the
        // rest of the app uses, then the settings with the hotkeys, then the tray (both in `setup`). The audio engine
        // follows once it exists.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            window::show_settings(app)
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::new().build())
        // Starting with the OS is off until the user turns it on; the entry starts the app hidden in the tray.
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![window::HIDDEN_ARG]),
        ))
        .manage::<SharedStatus>(Mutex::new(TrayStatus::default()))
        // Hotkeys and the tray drive this; the real engine takes its place once it exists.
        .manage::<SharedEngine>(Arc::new(NoEngine))
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::update_settings,
            commands::suspend_hotkeys
        ])
        .on_window_event(window::on_window_event)
        .setup(|app| {
            let handle = app.handle();
            // The settings decide whether this is the first run and which hotkeys to register, so they come first.
            service::install(handle);
            tray::setup(handle)?;
            let args: Vec<String> = std::env::args().collect();
            window::apply_startup_visibility(handle, &args);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Repeat that?! app");

    app.run(|_app, event| {
        // The window only hides, so the app never runs out of windows; keep it alive if something asks anyway. Quit
        // calls `exit` with a code and gets through.
        if let RunEvent::ExitRequested { code: None, api, .. } = event {
            api.prevent_exit();
        }
    });
}
