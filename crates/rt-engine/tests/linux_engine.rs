//! The engine end to end on Linux, against a real PipeWire: `pw-cat` plays the rt-core speech fixture, the engine
//! captures it through PipeWire, and a replay into an in-memory output plays its speech while the player is ducked.
//! Run it against a private, headless PipeWire with
//!
//! ```sh
//! eval "$(.github/scripts/start-pipewire.sh)"
//! cargo test -p rt-engine --test linux_engine -- --ignored --nocapture
//! ```
//!
//! The fixture is read from the repository and piped into the player; captured audio stays in memory.

#![cfg(target_os = "linux")]

#[path = "../../rt-capture/tests/pw_tools/mod.rs"]
mod pw_tools;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pw_tools::{pw_cat_raw_input, pw_dump};
use rt_capture::AppAudioSource;
use rt_capture::linux::PipeWireSource;
use rt_engine::{CaptureState, Engine, EngineConfig, EngineHandle, PlaybackState, Status, TargetApp, TestOutput};

/// What the player is called, with a media role of its own: WirePlumber remembers volumes by role.
const NAME: &str = "RT Engine Speech";

/// `pw-cat` playing the fixture once, then silence, so its stream stays. Killed when dropped.
struct Player {
    child: Child,
    feeder: Option<JoinHandle<()>>,
}

impl Player {
    fn start() -> Self {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rt-core/tests/fixtures/speech_8s.wav");
        let mut reader = hound::WavReader::open(&fixture).expect("the speech fixture");
        let spec = reader.spec();
        assert_eq!((spec.channels, spec.bits_per_sample), (1, 16), "{spec:?}");
        let speech: Vec<u8> = reader
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut child = Command::new("pw-cat")
            .args(["--playback", "--format", "f32", "--channels", "1", "--latency", "1024"])
            .args(pw_cat_raw_input())
            .args(["--rate", &spec.sample_rate.to_string()])
            .args(["--media-role", &format!("RtTestEngine{}", std::process::id())])
            .arg("-P")
            .arg(format!("{{ application.name = \"{NAME}\" }}"))
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pw-cat (pipewire-bin)");
        let mut stdin = child.stdin.take().unwrap();
        let silence = vec![0u8; spec.sample_rate as usize * 4 / 10];
        let feeder = Some(thread::spawn(move || {
            if stdin.write_all(&speech).is_ok() {
                while stdin.write_all(&silence).is_ok() {}
            }
        }));
        Self { child, feeder }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(feeder) = self.feeder.take() {
            let _ = feeder.join();
        }
    }
}

#[test]
#[ignore = "needs PipeWire"]
fn captures_replays_and_ducks_a_playing_app() {
    let source = Arc::new(PipeWireSource::new().expect("PipeWire runs"));
    let _player = Player::start();
    let app = wait_for_value(Duration::from_secs(10), || {
        source.list_apps().unwrap().into_iter().find(|app| app.name == NAME)
    })
    .expect("the player shows up in the app list");
    println!("player: {app:?}");

    let output = TestOutput::new(48_000);
    let engine = Engine::new(
        Arc::clone(&source) as Arc<dyn AppAudioSource>,
        Box::new(output.clone()),
        EngineConfig::default(),
    );
    let handle = engine.handle();
    handle.set_target(Some(TargetApp::from(&app)));
    wait_for(&handle, Duration::from_secs(10), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });

    // The fixture is 8 s of speech with short pauses: some of it is buffered once it has played.
    let status = wait_for(&handle, Duration::from_secs(20), "speech buffered", |s| {
        s.buffered >= Duration::from_secs(2)
    });
    println!("buffered: {status:?}");
    assert!(!status.vad_fallback, "Silero VAD loads");
    // Until the player is quiet and the last sentence is closed.
    thread::sleep(Duration::from_secs(8));
    let status = handle.status();
    println!(
        "after playing: {status:?}, {} samples dropped",
        handle.dropped_samples()
    );
    assert!(status.buffered >= Duration::from_secs(3), "{:?}", status.buffered);
    assert_eq!(handle.dropped_samples(), 0);

    // A replay ducks the player to 20%, plays speech, and gives the player its volume back once it ends.
    let before = volumes();
    assert!(!before.is_empty(), "the player has a stream");
    handle.replay();
    wait_for(&handle, Duration::from_secs(1), "a replay", |s| {
        s.playback != PlaybackState::Idle
    });
    let ducked = wait_for_value(Duration::from_secs(3), || {
        let now = volumes();
        let ducked = now.len() == before.len()
            && now
                .iter()
                .flatten()
                .zip(before.iter().flatten())
                .all(|(n, b)| (n - b * 0.2).abs() < 0.02);
        ducked.then_some(now)
    });
    println!("volume before {before:?}, during the replay {ducked:?}");
    assert!(ducked.is_some(), "ducked to 20% during the replay: {:?}", volumes());
    wait_for(&handle, Duration::from_secs(20), "the replay to end", |s| {
        s.playback == PlaybackState::Idle
    });
    let restored = wait_for_value(Duration::from_secs(3), || (volumes() == before).then_some(()));
    assert!(restored.is_some(), "restored after the replay: {:?}", volumes());

    let samples = output.samples();
    let loud = samples.iter().filter(|s| s.abs() > 0.02).count() as f32 / 48_000.0;
    println!(
        "replayed {:.2} s, {loud:.2} s of it loud",
        samples.len() as f32 / 48_000.0
    );
    assert!(loud > 0.3, "the replay is speech");
    drop(engine);
}

fn wait_for(handle: &EngineHandle, timeout: Duration, what: &str, done: impl Fn(&Status) -> bool) -> Status {
    wait_for_value(timeout, || Some(handle.status()).filter(|s| done(s)))
        .unwrap_or_else(|| panic!("timed out waiting for {what}; status: {:?}", handle.status()))
}

fn wait_for_value<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(value) = f() {
            return Some(value);
        }
        thread::sleep(Duration::from_millis(20));
    }
    None
}

/// The `channelVolumes` of each of the player's streams, read from `pw-dump`, independently of the backend.
fn volumes() -> Vec<Vec<f32>> {
    pw_dump()
        .iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with(":Node")))
        .filter(|o| o["info"]["props"]["application.name"].as_str() == Some(NAME))
        .filter_map(|o| {
            let channels = o["info"]["params"]["Props"]
                .as_array()?
                .iter()
                .find_map(|p| p["channelVolumes"].as_array())?;
            Some(channels.iter().filter_map(|c| c.as_f64()).map(|c| c as f32).collect())
        })
        .collect()
}
