//! Runs the real app binary with `--rt-self-check`, the way CI and the bundle checks do. The self-check ends before
//! Tauri starts, so this needs no display or audio device.

use std::process::{Command, Output};

use repeat_that_lib::self_check::MOCK_FALLBACK_VAR;

fn self_check(mock_fallback: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_repeat-that"));
    command.arg("--rt-self-check").env_remove(MOCK_FALLBACK_VAR);
    if mock_fallback {
        command.env(MOCK_FALLBACK_VAR, "1");
    }
    command.output().expect("the app binary starts")
}

/// The exit code, asserted first so that a failure shows what the self-check wrote on stderr (the load error).
fn assert_exit_code(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout: {:?}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The one JSON line on stdout.
fn report(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut lines = stdout.lines();
    let line = lines
        .next()
        .unwrap_or_else(|| panic!("the self-check prints a line\nstderr: {stderr}"));
    assert_eq!(lines.next(), None, "exactly one line, got {stdout:?}");
    serde_json::from_str(line).unwrap_or_else(|e| panic!("{line:?} is not JSON: {e}\nstderr: {stderr}"))
}

#[test]
fn silero_loads_so_it_exits_0() {
    let output = self_check(false);
    assert_exit_code(&output, 0);
    let report = report(&output);
    assert_eq!(report["vad"], "silero");
    assert_eq!(report["arch"], std::env::consts::ARCH);
    assert_eq!(report["os"], std::env::consts::OS);
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
}

/// The mock only exists in debug builds, so a release build has no way to reach the fallback from here.
#[cfg(debug_assertions)]
#[test]
fn the_fallback_exits_1() {
    let output = self_check(true);
    assert_exit_code(&output, 1);
    assert_eq!(report(&output)["vad"], "fallback");
}

/// A release build ignores the mock, so a stray variable can't make a shipped app report a fallback.
#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_ignores_the_mock() {
    let output = self_check(true);
    assert_exit_code(&output, 0);
    assert_eq!(report(&output)["vad"], "silero");
}
