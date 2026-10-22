//! Where captured audio comes from and where replays play: the OS capture backend and the sound card, or a WAV file
//! playing as a fake app and an output that plays into nothing, for development and tests. Debug builds can also
//! pretend that the OS doesn't let the app capture audio, see [`MockPermission`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rt_capture::{AppAudioSource, AudioApp, CaptureError, CaptureHandle, FrameSink, MockSource, PlatformOptions};
use rt_engine::{CpalOutput, NullOutput, OutputBackend};

use crate::locking::lock;

/// A WAV file to play as a fake app instead of capturing real apps, on any OS.
pub const MOCK_WAV_VAR: &str = "REPEAT_THAT_MOCK_WAV";
/// `1` plays replays into nothing, for machines without an audio device (CI, a dev box). Debug builds only.
pub const NULL_OUTPUT_VAR: &str = "REPEAT_THAT_NULL_OUTPUT";
/// `denied` makes captures fail as if the OS doesn't let the app capture audio, as macOS can, until "Ask again" in the
/// window allows it. For seeing and testing that state on any OS; debug builds only. See [`MockPermission`].
pub const MOCK_PERMISSION_VAR: &str = "REPEAT_THAT_MOCK_PERMISSION";
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
    /// The OS's capture backend: WASAPI process loopback on Windows, Core Audio process taps on macOS.
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
/// thread.
fn platform(duck_journal: Option<PathBuf>) -> Arc<dyn AppAudioSource> {
    set_up(move || {
        rt_capture::platform_source_with(PlatformOptions {
            duck_journal: duck_journal.clone(),
        })
    })
}

/// The source `setup` makes. A backend that can't be used on this OS (`Unsupported`) leaves the app with nothing to
/// capture, and the status says why. Any other failure, like a permission the user hasn't given yet, is tried again on
/// the next use, so capture starts as soon as the cause is gone.
fn set_up(
    setup: impl Fn() -> rt_capture::Result<Box<dyn AppAudioSource>> + Send + Sync + 'static,
) -> Arc<dyn AppAudioSource> {
    match setup() {
        Ok(source) => Arc::from(source),
        // Already a sentence for the user.
        Err(CaptureError::Unsupported(why)) => {
            eprintln!("audio capture is not supported: {why}");
            Arc::new(Unsupported(why))
        }
        Err(err) => {
            eprintln!("could not set up audio capture, trying again on the next use: {err}");
            Arc::new(Retrying {
                setup: Box::new(setup),
                source: Mutex::new(None),
            })
        }
    }
}

/// Makes the source of a backend that failed to set up, on the first use after that works. The engine's reconnect poll
/// and the app list commands are those uses, and the error they get is the one of the new attempt.
struct Retrying {
    setup: Box<dyn Fn() -> rt_capture::Result<Box<dyn AppAudioSource>> + Send + Sync>,
    source: Mutex<Option<Arc<dyn AppAudioSource>>>,
}

impl Retrying {
    /// The source, set up now if it isn't yet. Setting up can block, like the source's own calls do.
    fn get(&self) -> rt_capture::Result<Arc<dyn AppAudioSource>> {
        let mut source = lock(&self.source);
        if let Some(source) = source.as_ref() {
            return Ok(Arc::clone(source));
        }
        let made: Arc<dyn AppAudioSource> = Arc::from((self.setup)()?);
        *source = Some(Arc::clone(&made));
        Ok(made)
    }
}

impl AppAudioSource for Retrying {
    fn list_apps(&self) -> rt_capture::Result<Vec<AudioApp>> {
        self.get()?.list_apps()
    }

    fn start(&self, app: &AudioApp, sink: FrameSink) -> rt_capture::Result<Box<dyn CaptureHandle>> {
        self.get()?.start(app, sink)
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> rt_capture::Result<()> {
        self.get()?.set_app_volume(app, level)
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

/// Whether to pretend the OS refuses capture: only in debug builds, and only when [`MOCK_PERMISSION_VAR`] is `denied`.
pub fn wants_mock_permission(debug_build: bool, value: Option<&OsStr>) -> bool {
    debug_build && value == Some(OsStr::new("denied"))
}

/// The pretend permission for this build and environment, see [`wants_mock_permission`]. `None` in release builds.
pub fn mock_permission_from_env() -> Option<SharedMockPermission> {
    wants_mock_permission(cfg!(debug_assertions), std::env::var_os(MOCK_PERMISSION_VAR).as_deref())
        .then(MockPermission::denied)
}

/// How the pretend OS says to allow capture: what macOS 15 says.
pub const MOCK_HOW_TO_ALLOW: &str =
    "Turn on Repeat That in System Settings > Privacy & Security > Screen & System Audio Recording.";

/// A permission to capture audio that the user hasn't given, for debug builds on any OS (see [`MOCK_PERMISSION_VAR`]).
/// A source behind [`gate`](Self::gate) refuses to start captures with [`CaptureError::PermissionDenied`] until
/// [`allow`](Self::allow), which is what "Ask again" in the window does here, as if the user chose Allow at once.
/// Listing apps and ducking work as they do on macOS without the permission.
#[derive(Debug, Default)]
pub struct MockPermission {
    allowed: AtomicBool,
}

/// The managed state the permission commands find the pretend permission in, when there is one.
pub type SharedMockPermission = Arc<MockPermission>;

impl MockPermission {
    pub fn denied() -> SharedMockPermission {
        Arc::new(MockPermission::default())
    }

    pub fn allow(&self) {
        self.allowed.store(true, Ordering::Relaxed);
    }

    pub fn is_allowed(&self) -> bool {
        self.allowed.load(Ordering::Relaxed)
    }

    /// `source`, refusing to start captures until this is allowed.
    pub fn gate(self: &Arc<Self>, source: Arc<dyn AppAudioSource>) -> Arc<dyn AppAudioSource> {
        Arc::new(Gated {
            source,
            permission: Arc::clone(self),
        })
    }
}

/// A source whose captures need a [`MockPermission`].
struct Gated {
    source: Arc<dyn AppAudioSource>,
    permission: SharedMockPermission,
}

impl AppAudioSource for Gated {
    fn list_apps(&self) -> rt_capture::Result<Vec<AudioApp>> {
        self.source.list_apps()
    }

    fn start(&self, app: &AudioApp, sink: FrameSink) -> rt_capture::Result<Box<dyn CaptureHandle>> {
        if !self.permission.is_allowed() {
            return Err(CaptureError::PermissionDenied(MOCK_HOW_TO_ALLOW.into()));
        }
        self.source.start(app, sink)
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> rt_capture::Result<()> {
        self.source.set_app_volume(app, level)
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

    /// A setup that fails with `error` until its `grant`th attempt, then makes a fake app. Counts the attempts.
    fn setup_granted_at(
        grant: usize,
        error: fn() -> CaptureError,
    ) -> (
        impl Fn() -> rt_capture::Result<Box<dyn AppAudioSource>> + Send + Sync + 'static,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        let setup = move || {
            if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 < grant {
                return Err(error());
            }
            Ok(Box::new(MockSource::from_wav(DEBUG_FIXTURE.unwrap()).unwrap()) as Box<dyn AppAudioSource>)
        };
        (setup, attempts)
    }

    fn attempts(counter: &std::sync::atomic::AtomicUsize) -> usize {
        counter.load(std::sync::atomic::Ordering::SeqCst)
    }

    const ALLOW: &str = "Allow Repeat That to record audio in System Settings";

    fn denied() -> CaptureError {
        CaptureError::PermissionDenied(ALLOW.into())
    }

    #[test]
    fn a_setup_that_is_refused_is_tried_again_on_every_use_until_it_works() {
        let (setup, tries) = setup_granted_at(4, denied);
        let app = AudioApp {
            pid: 1,
            name: "Discord".into(),
            exe_path: None,
        };
        let source = set_up(setup);
        assert_eq!(attempts(&tries), 1, "set up once at launch");
        // Every use is a new attempt that says what is wrong, so capture resumes once the user has allowed it.
        match source.list_apps() {
            Err(CaptureError::PermissionDenied(how)) => assert_eq!(how, ALLOW),
            other => panic!("expected PermissionDenied, got {:?}", other.map(drop)),
        }
        assert!(matches!(
            source.start(&app, Box::new(|_, _| {})),
            Err(CaptureError::PermissionDenied(_))
        ));
        assert_eq!(attempts(&tries), 3);
        assert_eq!(source.list_apps().unwrap().len(), 1, "allowed on the fourth attempt");
        assert_eq!(source.list_apps().unwrap().len(), 1);
        assert_eq!(attempts(&tries), 4, "a source that works is kept");
    }

    #[test]
    fn other_setup_errors_than_unsupported_are_tried_again_too() {
        let (setup, tries) = setup_granted_at(2, || CaptureError::Backend("the audio service isn't up yet".into()));
        let source = set_up(setup);
        assert!(matches!(source.list_apps(), Ok(apps) if apps.len() == 1));
        assert_eq!(attempts(&tries), 2);
    }

    #[test]
    fn an_unsupported_setup_is_final() {
        let (setup, tries) = setup_granted_at(usize::MAX, || CaptureError::Unsupported("no PipeWire here".into()));
        let source = set_up(setup);
        for _ in 0..2 {
            match source.list_apps() {
                Err(CaptureError::Unsupported(why)) => assert_eq!(why, "no PipeWire here"),
                other => panic!("expected Unsupported, got {:?}", other.map(drop)),
            }
        }
        assert_eq!(
            attempts(&tries),
            1,
            "the engine doesn't retry it, and neither does the source"
        );
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_captures_through_its_own_backend() {
        const { assert!(rt_capture::HAS_PLATFORM_BACKEND) };
        // Also in a debug build, which has the fixture to play.
        assert_eq!(
            choose(rt_capture::HAS_PLATFORM_BACKEND, None, DEBUG_FIXTURE),
            SourceKind::Platform
        );
    }

    #[test]
    fn the_mock_permission_only_in_debug_builds_and_only_when_denied() {
        assert!(wants_mock_permission(true, os("denied")));
        assert!(!wants_mock_permission(false, os("denied")), "release builds ignore it");
        assert!(!wants_mock_permission(true, None));
        assert!(!wants_mock_permission(true, os("")), "an empty variable is ignored");
        assert!(!wants_mock_permission(true, os("allowed")));
        assert!(!wants_mock_permission(true, os("Denied")));
    }

    #[test]
    fn a_mock_permission_refuses_captures_until_allowed() {
        let permission = MockPermission::denied();
        let source = permission.gate(Arc::new(MockSource::from_wav(DEBUG_FIXTURE.unwrap()).unwrap()));
        // The app is listed, as macOS lists apps without the permission; only capturing it is refused.
        let app = source.list_apps().unwrap().remove(0);
        match source.start(&app, Box::new(|_, _| {})) {
            Err(CaptureError::PermissionDenied(how)) => assert_eq!(how, MOCK_HOW_TO_ALLOW),
            other => panic!("expected PermissionDenied, got {:?}", other.map(drop)),
        }
        assert!(source.set_app_volume(&app, 0.2).is_ok());

        permission.allow();
        let capture = source.start(&app, Box::new(|_, _| {})).unwrap();
        assert!(capture.is_running());
        capture.stop();
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
