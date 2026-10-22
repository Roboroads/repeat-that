// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    // Must stay first: when Windows started this process as a duck guard (a tiny watcher that restores ducked volumes
    // after a crash), it runs the guard and must exit before Tauri and single-instance start.
    if rt_capture::duck_guard::run_if_requested() {
        return ExitCode::SUCCESS;
    }
    // `--rt-self-check` prints which speech detector loaded and exits; it, too, must end before Tauri starts.
    if let Some(code) = repeat_that_lib::self_check::run_if_requested() {
        return code;
    }
    // `--rt-host-env` prints the environment host programs get from an AppImage, for CI; also before Tauri starts.
    if let Some(code) = repeat_that_lib::host_env::run_if_requested() {
        return code;
    }
    repeat_that_lib::run();
    ExitCode::SUCCESS
}
