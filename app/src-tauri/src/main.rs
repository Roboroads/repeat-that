// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Must stay first: when Windows started this process as a duck guard (a tiny watcher that restores ducked volumes
    // after a crash), it runs the guard and must exit before Tauri and single-instance start.
    if rt_capture::duck_guard::run_if_requested() {
        return;
    }
    repeat_that_lib::run();
}
