//! Where captured audio comes from and where replays play: the OS capture backend and the sound card, or a WAV file
//! playing as a fake app and an output that plays into nothing, for development and tests.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rt_capture::{AppAudioSource, AudioApp, CaptureError, CaptureHandle, FrameSink, MockSource};
use rt_engine::{CpalOutput, NullOutput, OutputBackend};

/// A WAV file to play as a fake app instead of capturing real apps, on any OS.
pub const MOCK_WAV_VAR: &str = "REPEAT_THAT_MOCK_WAV";
/// `1` plays replays into nothing, for machines without an audio device (CI, a dev box). Debug builds only.
pub const NULL_OUTPUT_VAR: &str = "REPEAT_THAT_NULL_OUTPUT";
/// What capture says on an OS without a capture backend.
pub const UNSUPPORTED: &str = "Capturing apps isn't supported on this OS yet";

/// The rt-core speech fixture, which debug builds play as a fake app where there is no capture backend. Release builds
/// don't know the path: it only exists in a checkout of the repository.
#[cfg(debug_assertions)]
const DEBUG_FIXTURE: Option<&str> = Some(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/rt-core/tests/fixtures/speech_8s.wav"
));
#[cfg(not(debug_assertions))]
const DEBUG_FIXTURE: Option<&str> = None;

/// Which capture source to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// WASAPI process loopback (Windows).
    Wasapi,
    /// A WAV file playing as a fake app, named after the file.
    Mock(PathBuf),
    /// Nothing can be captured; the message says why.
    Unsupported(String),
}

/// Picks the source. A WAV in `mock_wav` wins everywhere; otherwise the OS backend if there is one (`has_backend`),
/// else the `debug_fixture` (debug builds have one), else nothing. An empty variable counts as unset.
pub fn choose(has_backend: bool, mock_wav: Option<&OsStr>, debug_fixture: Option<&str>) -> SourceKind {
    if let Some(wav) = mock_wav.filter(|wav| !wav.is_empty()) {
        return SourceKind::Mock(PathBuf::from(wav));
    }
    if has_backend {
        return SourceKind::Wasapi;
    }
    match debug_fixture {
        Some(fixture) => SourceKind::Mock(PathBuf::from(fixture)),
        None => SourceKind::Unsupported(UNSUPPORTED.into()),
    }
}

/// [`choose`] for this build, OS and environment.
pub fn from_env() -> SourceKind {
    choose(cfg!(windows), std::env::var_os(MOCK_WAV_VAR).as_deref(), DEBUG_FIXTURE)
}

/// Makes the source. `duck_journal` is where the Windows backend notes the volumes it lowered, so they come back after
/// a crash; it holds volumes only, never audio. A WAV that can't be read leaves the app with nothing to capture, and
/// the status says why.
pub fn build(kind: SourceKind, duck_journal: Option<PathBuf>) -> Arc<dyn AppAudioSource> {
    match kind {
        SourceKind::Wasapi => wasapi(duck_journal),
        SourceKind::Mock(path) => match MockSource::from_wav(&path) {
            Ok(mock) => Arc::new(mock),
            Err(err) => {
                eprintln!("could not play {} as a fake app: {err}", path.display());
                Arc::new(Unsupported(format!("Could not play {}: {err}", file_name(&path))))
            }
        },
        SourceKind::Unsupported(why) => Arc::new(Unsupported(why)),
    }
}

/// The WASAPI source with a duck journal and guard. Restoring what an earlier run's journal lists talks to the audio
/// service and sets up COM for the thread, so it runs on a thread of its own rather than the UI thread.
#[cfg(windows)]
fn wasapi(duck_journal: Option<PathBuf>) -> Arc<dyn AppAudioSource> {
    let build = move || {
        let source = rt_capture::WasapiSource::new();
        match duck_journal {
            // The guard reads the journal to know what to restore, so there is no guard without one.
            Some(journal) => source.with_duck_journal(journal).with_duck_guard(),
            None => source,
        }
    };
    let source = std::thread::Builder::new()
        .name("rt-source".into())
        .spawn(build)
        .ok()
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    Arc::new(source)
}

#[cfg(not(windows))]
fn wasapi(_duck_journal: Option<PathBuf>) -> Arc<dyn AppAudioSource> {
    Arc::new(Unsupported(UNSUPPORTED.into()))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Whether replays play into nothing: only in debug builds, and only when [`NULL_OUTPUT_VAR`] is `1`.
pub fn wants_null_output(debug_build: bool, value: Option<&OsStr>) -> bool {
    debug_build && value == Some(OsStr::new("1"))
}

/// The output for this build and environment, see [`wants_null_output`].
pub fn output_from_env() -> Box<dyn OutputBackend> {
    if wants_null_output(cfg!(debug_assertions), std::env::var_os(NULL_OUTPUT_VAR).as_deref()) {
        Box::new(NullOutput::default())
    } else {
        Box::new(CpalOutput::new())
    }
}

/// A source that can't capture anything, with the reason.
struct Unsupported(String);

impl Unsupported {
    fn error(&self) -> CaptureError {
        CaptureError::Unsupported(self.0.clone())
    }
}

impl AppAudioSource for Unsupported {
    fn list_apps(&self) -> rt_capture::Result<Vec<AudioApp>> {
        Err(self.error())
    }

    fn start(&self, _app: &AudioApp, _sink: FrameSink) -> rt_capture::Result<Box<dyn CaptureHandle>> {
        Err(self.error())
    }

    fn set_app_volume(&self, _app: &AudioApp, _level: f32) -> rt_capture::Result<()> {
        Err(self.error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "/repo/speech_8s.wav";

    fn os(value: &str) -> Option<&OsStr> {
        Some(OsStr::new(value))
    }

    #[test]
    fn windows_captures_real_apps() {
        assert_eq!(choose(true, None, None), SourceKind::Wasapi);
        assert_eq!(choose(true, None, Some(FIXTURE)), SourceKind::Wasapi);
    }

    #[test]
    fn a_mock_wav_wins_everywhere() {
        let mock = SourceKind::Mock(PathBuf::from("/tmp/call.wav"));
        assert_eq!(choose(true, os("/tmp/call.wav"), None), mock);
        assert_eq!(choose(false, os("/tmp/call.wav"), Some(FIXTURE)), mock);
        assert_eq!(choose(false, os("/tmp/call.wav"), None), mock);
    }

    #[test]
    fn an_empty_mock_variable_is_ignored() {
        assert_eq!(choose(true, os(""), None), SourceKind::Wasapi);
        assert_eq!(choose(false, os(""), None), SourceKind::Unsupported(UNSUPPORTED.into()));
    }

    #[test]
    fn debug_builds_elsewhere_play_the_fixture() {
        assert_eq!(
            choose(false, None, Some(FIXTURE)),
            SourceKind::Mock(PathBuf::from(FIXTURE))
        );
    }

    #[test]
    fn release_builds_elsewhere_capture_nothing() {
        assert_eq!(choose(false, None, None), SourceKind::Unsupported(UNSUPPORTED.into()));
    }

    #[test]
    fn this_debug_build_knows_where_the_fixture_is() {
        let fixture = DEBUG_FIXTURE.expect("tests are debug builds");
        assert!(Path::new(fixture).is_file(), "{fixture}");
    }

    #[test]
    fn the_fixture_plays_as_a_fake_app_named_after_the_file() {
        let source = build(SourceKind::Mock(PathBuf::from(DEBUG_FIXTURE.unwrap())), None);
        let apps = source.list_apps().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "speech_8s");
        assert_eq!(apps[0].exe_path, None);
    }

    #[test]
    fn a_wav_that_cannot_be_read_captures_nothing_and_says_why() {
        let source = build(SourceKind::Mock(PathBuf::from("/does/not/exist/call.wav")), None);
        match source.list_apps() {
            Err(CaptureError::Unsupported(why)) => assert!(why.contains("call.wav"), "{why}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn the_unsupported_source_refuses_everything_with_its_reason() {
        let source = build(SourceKind::Wasapi, None);
        let app = AudioApp {
            pid: 1,
            name: "Discord".into(),
            exe_path: None,
        };
        let reason = |result: rt_capture::Result<()>| match result {
            Err(CaptureError::Unsupported(why)) => why,
            other => panic!("expected Unsupported, got {other:?}"),
        };
        assert_eq!(reason(source.list_apps().map(drop)), UNSUPPORTED);
        assert_eq!(reason(source.start(&app, Box::new(|_, _| {})).map(drop)), UNSUPPORTED);
        assert_eq!(reason(source.set_app_volume(&app, 0.2)), UNSUPPORTED);
    }

    #[test]
    fn null_output_only_in_debug_builds_and_only_when_asked() {
        assert!(wants_null_output(true, os("1")));
        assert!(!wants_null_output(false, os("1")), "release builds always play");
        assert!(!wants_null_output(true, None));
        assert!(!wants_null_output(true, os("0")));
        assert!(!wants_null_output(true, os("")));
    }
}
