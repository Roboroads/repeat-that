//! Where captured audio comes from and where replays play: the OS capture backend and the sound card, or a WAV file
//! playing as a fake app and an output that plays into nothing, for development and tests.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rt_capture::{AppAudioSource, AudioApp, CaptureError, CaptureHandle, FrameSink, MockSource, PlatformOptions};
use rt_engine::{CpalOutput, NullOutput, OutputBackend};

use crate::locking::lock;

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
    /// The OS's capture backend: WASAPI process loopback on Windows.
    Platform,
    /// A WAV file playing as a fake app, named after the file.
    Mock(PathBuf),
    /// Nothing can be captured; the message says why.
    Unsupported(String),
}

/// Picks the source. A WAV in `mock_wav` wins everywhere; otherwise the OS backend if there is one (`has_backend`, see
/// [`rt_capture::HAS_PLATFORM_BACKEND`]), else the `debug_fixture` (debug builds have one), else nothing. An empty
/// variable counts as unset.
pub fn choose(has_backend: bool, mock_wav: Option<&OsStr>, debug_fixture: Option<&str>) -> SourceKind {
    if let Some(wav) = mock_wav.filter(|wav| !wav.is_empty()) {
        return SourceKind::Mock(PathBuf::from(wav));
    }
    if has_backend {
        return SourceKind::Platform;
    }
    match debug_fixture {
        Some(fixture) => SourceKind::Mock(PathBuf::from(fixture)),
        None => SourceKind::Unsupported(UNSUPPORTED.into()),
    }
}

/// [`choose`] for this build, OS and environment.
pub fn from_env() -> SourceKind {
    choose(
        rt_capture::HAS_PLATFORM_BACKEND,
        std::env::var_os(MOCK_WAV_VAR).as_deref(),
        DEBUG_FIXTURE,
    )
}

/// The source, made on a thread of its own right away (see [`LazySource`]). `duck_journal` is where the OS backend
/// notes the volumes it lowered, so they come back after a crash; it holds volumes only, never audio. A WAV that can't
/// be read leaves the app with nothing to capture, and the status says why.
pub fn build(kind: SourceKind, duck_journal: Option<PathBuf>) -> Arc<dyn AppAudioSource> {
    let source = Arc::new(LazySource::new(move || make(kind, duck_journal)));
    make_early(&source);
    source
}

fn make(kind: SourceKind, duck_journal: Option<PathBuf>) -> Arc<dyn AppAudioSource> {
    match kind {
        SourceKind::Platform => platform(duck_journal),
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

/// The OS backend with a duck journal (and on Windows a guard). Restoring what an earlier run's journal lists talks to
/// the audio service, and on Windows sets up COM for the thread, which is why [`LazySource`] makes it away from the UI
/// thread. A backend that can't be set up leaves the app with nothing to capture, and the status says why.
fn platform(duck_journal: Option<PathBuf>) -> Arc<dyn AppAudioSource> {
    match rt_capture::platform_source_with(PlatformOptions { duck_journal }) {
        Ok(source) => Arc::from(source),
        Err(err) => {
            eprintln!("could not set up audio capture: {err}");
            Arc::new(Unsupported(match err {
                // Already a sentence for the user.
                CaptureError::Unsupported(why) => why,
                other => other.to_string(),
            }))
        }
    }
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

/// Makes a source.
type MakeSource = Box<dyn FnOnce() -> Arc<dyn AppAudioSource> + Send>;

/// A source made on first use. Making one can block (the Windows backend first gives back the volumes an earlier run
/// left lowered), and the engine's threads and the commands that list apps are the only users, so the UI thread never
/// waits for it. Users that come while it is being made wait for that one.
pub struct LazySource {
    make: Mutex<Option<MakeSource>>,
    source: OnceLock<Arc<dyn AppAudioSource>>,
}

impl LazySource {
    pub fn new(make: impl FnOnce() -> Arc<dyn AppAudioSource> + Send + 'static) -> Self {
        LazySource {
            make: Mutex::new(Some(Box::new(make))),
            source: OnceLock::new(),
        }
    }

    fn get(&self) -> &dyn AppAudioSource {
        self.source
            .get_or_init(|| {
                let make = lock(&self.make).take().expect("a lazy source is made once");
                make()
            })
            .as_ref()
    }
}

impl AppAudioSource for LazySource {
    fn list_apps(&self) -> rt_capture::Result<Vec<AudioApp>> {
        self.get().list_apps()
    }

    fn start(&self, app: &AudioApp, sink: FrameSink) -> rt_capture::Result<Box<dyn CaptureHandle>> {
        self.get().start(app, sink)
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> rt_capture::Result<()> {
        self.get().set_app_volume(app, level)
    }
}

/// Makes `source` now on a thread of its own and returns at once, so an earlier run's lowered volumes come back at
/// launch even before anything is captured.
pub fn make_early(source: &Arc<LazySource>) {
    let source = Arc::clone(source);
    let spawned = std::thread::Builder::new().name("rt-source".into()).spawn(move || {
        source.get();
    });
    if let Err(err) = spawned {
        // The first user makes it instead.
        eprintln!("could not start making the audio source: {err}");
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
    fn the_platform_backend_wins_without_a_mock() {
        assert_eq!(choose(true, None, None), SourceKind::Platform);
        assert_eq!(choose(true, None, Some(FIXTURE)), SourceKind::Platform);
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
        assert_eq!(choose(true, os(""), None), SourceKind::Platform);
        assert_eq!(choose(false, os(""), None), SourceKind::Unsupported(UNSUPPORTED.into()));
    }

    #[test]
    fn debug_builds_without_a_backend_play_the_fixture() {
        assert_eq!(
            choose(false, None, Some(FIXTURE)),
            SourceKind::Mock(PathBuf::from(FIXTURE))
        );
    }

    #[test]
    fn release_builds_without_a_backend_capture_nothing() {
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

    #[test]
    fn without_a_backend_the_platform_source_refuses_everything_with_its_reason() {
        // On OSes with a backend this would talk to the real audio service.
        if rt_capture::HAS_PLATFORM_BACKEND {
            return;
        }
        let reason_of_the_backend = match rt_capture::platform_source() {
            Err(CaptureError::Unsupported(why)) => why,
            _ => panic!("expected Unsupported"),
        };
        let source = build(SourceKind::Platform, None);
        let app = AudioApp {
            pid: 1,
            name: "Discord".into(),
            exe_path: None,
        };
        let reason = |result: rt_capture::Result<()>| match result {
            Err(CaptureError::Unsupported(why)) => why,
            other => panic!("expected Unsupported, got {other:?}"),
        };
        assert_eq!(reason(source.list_apps().map(drop)), reason_of_the_backend);
        assert_eq!(
            reason(source.start(&app, Box::new(|_, _| {})).map(drop)),
            reason_of_the_backend
        );
        assert_eq!(reason(source.set_app_volume(&app, 0.2)), reason_of_the_backend);
    }

    /// A source that counts how often it was made, and waits for `release` while it is made, if one is given.
    fn counted(
        release: Option<std::sync::mpsc::Receiver<()>>,
    ) -> (Arc<LazySource>, Arc<std::sync::atomic::AtomicUsize>) {
        let made = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&made);
        let source = LazySource::new(move || {
            if let Some(release) = release {
                let _ = release.recv();
            }
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Arc::new(MockSource::from_wav(DEBUG_FIXTURE.unwrap()).unwrap()) as Arc<dyn AppAudioSource>
        });
        (Arc::new(source), made)
    }

    #[test]
    fn a_lazy_source_is_made_on_first_use_and_only_once() {
        let (source, made) = counted(None);
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 0, "not made up front");
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let source = Arc::clone(&source);
                std::thread::spawn(move || source.list_apps().unwrap())
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap()[0].name, "speech_8s");
        }
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn making_the_source_early_does_not_wait_for_it() {
        // The app starts making it at launch, on the UI thread, and must go on while the audio service is slow.
        let (release, wait) = std::sync::mpsc::channel();
        let (source, made) = counted(Some(wait));
        make_early(&source);
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 0);
        release.send(()).unwrap();
        // The first use waits for the one being made, and doesn't make another.
        assert_eq!(source.list_apps().unwrap().len(), 1);
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 1);
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
