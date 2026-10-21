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

/// The one JSON line on stdout.
fn report(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    let mut lines = stdout.lines();
    let line = lines.next().expect("the self-check prints a line");
    assert_eq!(lines.next(), None, "exactly one line, got {stdout:?}");
    serde_json::from_str(line).unwrap_or_else(|e| panic!("{line:?} is not JSON: {e}"))
}

#[test]
fn silero_loads_so_it_exits_0() {
    let output = self_check(false);
    let report = report(&output);
    assert_eq!(report["vad"], "silero");
    assert_eq!(report["arch"], std::env::consts::ARCH);
    assert_eq!(report["os"], std::env::consts::OS);
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_fallback_exits_1() {
    let output = self_check(true);
    assert_eq!(report(&output)["vad"], "fallback");
    assert_eq!(output.status.code(), Some(1));
}
