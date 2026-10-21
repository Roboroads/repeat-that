//! The engine end to end on Windows, against a real audio device: a child process plays the rt-core speech fixture, the
//! engine captures it with WASAPI, and a replay into an in-memory output plays its speech while the player is ducked.
//! CI installs a virtual audio device for this. Run it locally with
//!
//! ```sh
//! cargo test -p rt-engine --test windows_engine -- --ignored --nocapture
//! ```
//!
//! The fixture is only read by the player; captured audio stays in memory.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rt_capture::AppAudioSource;
use rt_capture::windows::WasapiSource;
use rt_engine::{CaptureState, Engine, EngineConfig, EngineHandle, PlaybackState, Status, TargetApp, TestOutput};

/// A child process that is killed when the test ends, also on a failed assertion.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Plays the fixture once through `System.Media.SoundPlayer`, then keeps its audio session alive.
fn start_player() -> Killed {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(r"..\rt-core\tests\fixtures\speech_8s.wav");
    assert!(fixture.exists(), "{} is missing", fixture.display());
    let script = format!(
        "(New-Object Media.SoundPlayer('{}')).PlaySync(); Start-Sleep 300",
        fixture.display()
    );
    Killed(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .spawn()
            .expect("start PowerShell"),
    )
}

#[test]
#[ignore = "plays audio; needs an audio output device"]
fn captures_replays_and_ducks_a_playing_app() {
    let source = Arc::new(WasapiSource::new());
    let player = start_player();
    let pid = player.0.id();
    let app = wait_for_value(Duration::from_secs(30), || {
        source.list_apps().unwrap().into_iter().find(|app| app.pid == pid)
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
    thread::sleep(Duration::from_secs(10));
    let status = handle.status();
    println!(
        "after playing: {status:?}, {} samples dropped",
        handle.dropped_samples()
    );
    assert!(status.buffered >= Duration::from_secs(3), "{:?}", status.buffered);
    assert_eq!(handle.dropped_samples(), 0);

    // A replay ducks the player to 20%, plays speech, and gives the player its volume back once it ends.
    let before = session_volumes(pid);
    assert!(!before.is_empty(), "the player has an audio session");
    handle.replay();
    wait_for(&handle, Duration::from_secs(1), "a replay", |s| {
        s.playback != PlaybackState::Idle
    });
    let ducked = wait_for_value(Duration::from_secs(2), || {
        let now = session_volumes(pid);
        let ducked = now.len() == before.len() && now.iter().zip(&before).all(|(n, b)| (n - b * 0.2).abs() < 0.02);
        ducked.then_some(now)
    });
    println!("volume before {before:?}, during the replay {ducked:?}");
    assert!(
        ducked.is_some(),
        "ducked to 20% during the replay: {:?}",
        session_volumes(pid)
    );
    wait_for(&handle, Duration::from_secs(20), "the replay to end", |s| {
        s.playback == PlaybackState::Idle
    });
    let restored = wait_for_value(Duration::from_secs(3), || {
        (session_volumes(pid) == before).then_some(())
    });
    assert!(
        restored.is_some(),
        "restored after the replay: {:?}",
        session_volumes(pid)
    );

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

/// The volume of each audio session of `pid`, read independently of the backend.
fn session_volumes(pid: u32) -> Vec<f32> {
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
        MMDeviceEnumerator, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::core::Interface;

    let mut volumes = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).unwrap();
        for d in 0..devices.GetCount().unwrap() {
            let manager: IAudioSessionManager2 = devices.Item(d).unwrap().Activate(CLSCTX_ALL, None).unwrap();
            let sessions = manager.GetSessionEnumerator().unwrap();
            for s in 0..sessions.GetCount().unwrap() {
                let session: IAudioSessionControl2 = sessions.GetSession(s).unwrap().cast().unwrap();
                if session.GetProcessId().ok() == Some(pid) {
                    volumes.push(session.cast::<ISimpleAudioVolume>().unwrap().GetMasterVolume().unwrap());
                }
            }
        }
    }
    volumes
}
