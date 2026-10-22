//! The hidden `--rt-self-check` argument: prints one JSON line saying which speech detector loaded, and exits.
//!
//! `{"vad":"silero"|"fallback","arch":..,"os":..,"version":..,"updater":true|false}`, the last one being whether the
//! build has an update key compiled in. The exit code is 0 only when Silero loaded, so a
//! script (CI, a bundle check) can tell a working install from one that fell back to the energy detector. It loads
//! Silero the way the engine does, runs one frame of silence through it, and plays and captures nothing.
//!
//! The app is its own self-check, so `main` must call [`run_if_requested`] early, before Tauri starts. A Windows
//! release build has no console (`windows_subsystem = "windows"`), so the line is never printed there and the exit
//! code is all a caller sees. Run it with `start /wait repeat-that.exe --rt-self-check` and read `%ERRORLEVEL%`, or
//! with `Start-Process -Wait -PassThru repeat-that.exe --rt-self-check` in PowerShell and read `ExitCode`.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::process::ExitCode;

use rt_core::SileroVad;
use serde::Serialize;

/// The hidden argument that makes the app run the self-check.
const ARG: &str = "--rt-self-check";
/// `1` makes the self-check report the fallback without loading Silero. Debug builds only, so tests can reach the
/// failing path on any OS.
pub const MOCK_FALLBACK_VAR: &str = "REPEAT_THAT_MOCK_VAD_FALLBACK";

/// Which speech detector the app would use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Detector {
    /// Silero VAD loaded.
    Silero,
    /// It didn't, so the app would use the simpler energy detector.
    Fallback,
}

/// What the self-check prints, as one JSON line. The fields are in this order.
#[derive(Debug, Serialize)]
pub struct Report {
    vad: Detector,
    arch: &'static str,
    os: &'static str,
    version: &'static str,
    /// Whether the build has an update key compiled in, which is what makes the updater exist.
    updater: bool,
}

impl Report {
    pub fn new(vad: Detector) -> Self {
        Report {
            vad,
            arch: std::env::consts::ARCH,
            os: std::env::consts::OS,
            version: env!("CARGO_PKG_VERSION"),
            updater: crate::updater::is_compiled_in(),
        }
    }

    /// The report as one line of JSON, without a trailing newline.
    pub fn json_line(&self) -> String {
        serde_json::to_string(self).expect("a report of plain strings serializes")
    }

    /// 0 when Silero loaded, 1 otherwise.
    pub fn exit_code(&self) -> u8 {
        match self.vad {
            Detector::Silero => 0,
            Detector::Fallback => 1,
        }
    }
}

/// Runs the self-check if this process was started with [`ARG`], and returns the exit code once it is done: the
/// caller should then exit with it. Returns `None` right away otherwise.
pub fn run_if_requested() -> Option<ExitCode> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if !requested(&args) {
        return None;
    }
    let mock = wants_mock_fallback(cfg!(debug_assertions), std::env::var_os(MOCK_FALLBACK_VAR).as_deref());
    let report = Report::new(detect(mock));
    // A closed stdout (no console) must not change the exit code.
    let _ = writeln!(std::io::stdout().lock(), "{}", report.json_line());
    Some(ExitCode::from(report.exit_code()))
}

/// Whether `args` (without the program name) ask for the self-check.
fn requested(args: &[OsString]) -> bool {
    matches!(args, [flag] if flag == OsStr::new(ARG))
}

/// Whether to report the fallback without trying Silero: only in debug builds, and only when [`MOCK_FALLBACK_VAR`] is
/// `1`.
fn wants_mock_fallback(debug_build: bool, value: Option<&OsStr>) -> bool {
    debug_build && value == Some(OsStr::new("1"))
}

/// Loads Silero and runs a frame of silence through it, or reports why it didn't on stderr (stdout is the JSON line
/// only). Loading alone isn't enough: ONNX Runtime can accept the model and still fail to run it.
fn detect(mock_fallback: bool) -> Detector {
    if mock_fallback {
        eprintln!("{MOCK_FALLBACK_VAR} is set, so Silero VAD is not loaded");
        return Detector::Fallback;
    }
    classify(run_silero())
}

/// Loads Silero and returns the probability it gives a frame of silence, or what went wrong.
fn run_silero() -> Result<f32, String> {
    let mut vad = SileroVad::new().map_err(|e| format!("Silero VAD failed to load: {e}"))?;
    vad.self_test()
        .map_err(|e| format!("Silero VAD loaded but failed to run: {e}"))
}

/// Silero counts as loaded only when the run succeeded and gave a probability.
fn classify(outcome: Result<f32, String>) -> Detector {
    match outcome {
        Ok(probability) if (0.0..=1.0).contains(&probability) => Detector::Silero,
        Ok(output) => {
            eprintln!("Silero VAD gave {output}, which is not a probability");
            Detector::Fallback
        }
        Err(why) => {
            eprintln!("{why}");
            Detector::Fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn json_line_lists_the_five_fields_in_order() {
        let line = Report::new(Detector::Silero).json_line();
        let expected = format!(
            r#"{{"vad":"silero","arch":"{}","os":"{}","version":"{}","updater":{}}}"#,
            std::env::consts::ARCH,
            std::env::consts::OS,
            env!("CARGO_PKG_VERSION"),
            crate::updater::is_compiled_in()
        );
        assert_eq!(line, expected);
        assert!(!line.contains('\n'));
    }

    #[test]
    fn the_report_says_whether_the_build_has_an_update_key() {
        let report = Report::new(Detector::Silero);
        let value: serde_json::Value = serde_json::from_str(&report.json_line()).unwrap();
        assert!(value["updater"].is_boolean(), "{value}");
        assert_eq!(value["updater"], crate::updater::is_compiled_in());
    }

    #[test]
    fn json_line_is_valid_json_for_both_detectors() {
        for (detector, name) in [(Detector::Silero, "silero"), (Detector::Fallback, "fallback")] {
            let value: serde_json::Value = serde_json::from_str(&Report::new(detector).json_line()).unwrap();
            assert_eq!(value["vad"], name);
            assert_eq!(value["arch"], std::env::consts::ARCH);
            assert_eq!(value["os"], std::env::consts::OS);
            assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
            assert!(value["updater"].is_boolean());
        }
    }

    #[test]
    fn exit_code_is_zero_only_for_silero() {
        assert_eq!(Report::new(Detector::Silero).exit_code(), 0);
        assert_eq!(Report::new(Detector::Fallback).exit_code(), 1);
    }

    #[test]
    fn only_the_exact_argument_asks_for_the_self_check() {
        assert!(requested(&os(&["--rt-self-check"])));
        assert!(!requested(&os(&[])));
        assert!(!requested(&os(&["--hidden"])));
        assert!(!requested(&os(&["--rt-self-check", "extra"])));
        assert!(!requested(&os(&["--rt-duck-guard", "journal", "1"])));
    }

    #[test]
    fn the_mock_fallback_needs_a_debug_build_and_the_value_1() {
        let one = OsStr::new("1");
        assert!(wants_mock_fallback(true, Some(one)));
        assert!(!wants_mock_fallback(false, Some(one)));
        assert!(!wants_mock_fallback(true, None));
        assert!(!wants_mock_fallback(true, Some(OsStr::new(""))));
        assert!(!wants_mock_fallback(true, Some(OsStr::new("0"))));
    }

    #[test]
    fn a_probability_from_the_model_means_silero() {
        for probability in [0.0, 0.03, 1.0] {
            assert_eq!(classify(Ok(probability)), Detector::Silero);
        }
    }

    #[test]
    fn a_failed_run_means_fallback() {
        assert_eq!(classify(Err("inference failed".into())), Detector::Fallback);
    }

    #[test]
    fn an_output_that_is_no_probability_means_fallback() {
        for output in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert_eq!(classify(Ok(output)), Detector::Fallback, "output {output}");
        }
    }

    #[test]
    fn a_mocked_fallback_does_not_load_silero() {
        assert_eq!(detect(true), Detector::Fallback);
    }

    #[test]
    fn silero_loads_on_this_os() {
        assert_eq!(detect(false), Detector::Silero);
    }
}
