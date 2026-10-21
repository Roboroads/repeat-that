//! End-to-end tests of the Linux backend against a real PipeWire: players play a tone, and we list, capture and duck
//! them, check that a microphone never ends up in a capture, and that the duck guard and the journal restore what a
//! killed process ducked. They need PipeWire and WirePlumber, and `pw-cat`, `pw-cli` and `pw-dump`. Run them against a
//! private, headless PipeWire with
//!
//! ```sh
//! eval "$(.github/scripts/start-pipewire.sh)"
//! cargo build -p rt-capture --example probe
//! cargo test -p rt-capture --test linux_audio -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The tones are made here in memory and piped into the players; nothing is written to disk but the probe's duck
//! journal, which holds volumes only. Each player has a media role of its own, which is what WirePlumber remembers its
//! volume by, so the tests don't change each other's volumes.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::f64::consts::TAU;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use std::{env, fs};

use pipewire as pw;
use rt_capture::linux::PipeWireSource;
use rt_capture::{AppAudioSource, AudioApp, CaptureFormat, CaptureHandle};
use serde_json::Value;

const TONE_HZ: f64 = 1000.0;
/// The players' sines have an amplitude of 0.5, so an RMS of about 0.35 at full volume.
const TONE_RMS: f64 = 0.354;
/// How far the compensated capture of a ducked app may stray from its level, in any 1 ms window: two fade steps of
/// 0.48 dB. What CI measured is in src/ramp.rs.
const MAX_OFF_DB: f64 = 1.0;
/// How many times the test fades down and back up.
const FADES: usize = 5;
const RATE: u32 = 48_000;

/// The tests share one PipeWire, so they take turns.
static AUDIO: Mutex<()> = Mutex::new(());

fn take_turn() -> std::sync::MutexGuard<'static, ()> {
    AUDIO.lock().unwrap_or_else(|e| e.into_inner())
}

/// Interleaved stereo `f32` of a sine at `hz` with amplitude 0.5, one second long, as bytes for a raw player.
fn sine_bytes(hz: f64) -> Vec<u8> {
    (0..RATE)
        .map(|i| (0.5 * (TAU * hz * f64::from(i) / f64::from(RATE)).sin()) as f32)
        .flat_map(|s| [s, s])
        .flat_map(f32::to_le_bytes)
        .collect()
}

/// A media role of its own for a player called `name` in this test run: WirePlumber remembers volumes by role, also
/// from one run to the next.
fn role(name: &str) -> String {
    format!("RtTest{}{}", name.replace(' ', ""), std::process::id())
}

/// `pw-cat` playing a tone made here, fed through its standard input. Killed when dropped.
struct Player {
    child: Child,
    feeder: Option<JoinHandle<()>>,
}

impl Player {
    fn start(name: &str) -> Self {
        Self::start_with(name, TONE_HZ, &[])
    }

    fn start_with(name: &str, hz: f64, extra: &[&str]) -> Self {
        let mut child = Command::new("pw-cat")
            .args([
                "--playback",
                "--raw",
                "--format",
                "f32",
                "--rate",
                "48000",
                "--channels",
                "2",
            ])
            .args(["--latency", "1024", "--media-role", &role(name)])
            .arg("-P")
            .arg(format!(
                "{{ application.name = \"{name}\" node.name = \"{}\" }}",
                role(name)
            ))
            .args(extra)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pw-cat (pipewire-bin)");
        let stdin = child.stdin.take().unwrap();
        let feeder = Some(thread::spawn(move || feed(stdin, &sine_bytes(hz))));
        Self { child, feeder }
    }

    /// `pacat` playing the tone through the PulseAudio API, as most apps do: pipewire-pulse plays it on its behalf.
    fn start_pulse(name: &str) -> Self {
        let mut child = Command::new("pacat")
            .args([
                "--playback",
                "--raw",
                "--format=float32le",
                "--rate=48000",
                "--channels=2",
            ])
            .arg(format!("--client-name={name}"))
            .arg(format!("--property=application.name={name}"))
            .arg(format!("--property=media.role={}", role(name)))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pacat (pulseaudio-utils)");
        let stdin = child.stdin.take().unwrap();
        let feeder = Some(thread::spawn(move || feed(stdin, &sine_bytes(TONE_HZ))));
        Self { child, feeder }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

/// Writes `second` over and over until the player is gone.
fn feed(mut stdin: ChildStdin, second: &[u8]) {
    while stdin.write_all(second).is_ok() {}
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

/// What `pw-dump` says now.
fn dump() -> Vec<Value> {
    let output = Command::new("pw-dump").output().expect("run pw-dump");
    serde_json::from_slice(&output.stdout).expect("pw-dump prints JSON")
}

fn props(object: &Value) -> &Value {
    &object["info"]["props"]
}

fn is(object: &Value, kind: &str) -> bool {
    object["type"].as_str().is_some_and(|t| t.ends_with(kind))
}

/// The ids of the nodes whose `key` property is `value`.
fn nodes_where(dump: &[Value], key: &str, value: &str) -> Vec<u64> {
    dump.iter()
        .filter(|o| is(o, ":Node") && props(o)[key].as_str() == Some(value))
        .filter_map(|o| o["id"].as_u64())
        .collect()
}

/// The `channelVolumes` of every playback stream of the app called `name`, read independently of the backend.
fn volumes(name: &str) -> Vec<Vec<f32>> {
    let dump = dump();
    dump.iter()
        .filter(|o| is(o, ":Node") && props(o)["application.name"].as_str() == Some(name))
        .filter(|o| props(o)["media.class"].as_str() == Some("Stream/Output/Audio"))
        .filter_map(|o| {
            let channels = o["info"]["params"]["Props"]
                .as_array()?
                .iter()
                .find_map(|p| p["channelVolumes"].as_array())?;
            Some(channels.iter().filter_map(|c| c.as_f64()).map(|c| c as f32).collect())
        })
        .collect()
}

/// Sets a node's volume the way a volume control would.
fn set_by_hand(node: u64, level: f32) {
    let status = Command::new("pw-cli")
        .args(["set-param", &node.to_string(), "Props"])
        .arg(format!("{{ channelVolumes: [ {level}, {level} ] }}"))
        .stdout(Stdio::null())
        .status()
        .expect("run pw-cli");
    assert!(status.success(), "pw-cli set-param: {status}");
}

fn close(a: &[Vec<f32>], b: &[Vec<f32>], tolerance: f32) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() <= tolerance))
}

fn scaled(volumes: &[Vec<f32>], level: f32) -> Vec<Vec<f32>> {
    volumes.iter().map(|v| v.iter().map(|c| c * level).collect()).collect()
}

fn wait_for<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(value) = f() {
            return Some(value);
        }
        thread::sleep(Duration::from_millis(50));
    }
    None
}

/// Waits until the app called `name` is in the list.
fn find_app(source: &dyn AppAudioSource, name: &str) -> AudioApp {
    wait_for(Duration::from_secs(10), || {
        source.list_apps().unwrap().into_iter().find(|app| app.name == name)
    })
    .unwrap_or_else(|| panic!("{name} shows up in the app list: {:?}", source.list_apps()))
}

/// Captured audio, all of it, as it arrived.
struct Recording {
    rx: mpsc::Receiver<(Vec<f32>, CaptureFormat)>,
}

fn record(source: &dyn AppAudioSource, app: &AudioApp) -> (Box<dyn CaptureHandle>, Recording) {
    let (tx, rx) = mpsc::channel();
    let handle = source
        .start(
            app,
            Box::new(move |frames, format| {
                let _ = tx.send((frames.to_vec(), format));
            }),
        )
        .expect("capture starts");
    (handle, Recording { rx })
}

impl Recording {
    /// The first channel of what arrived so far, and its rate.
    fn left(&self) -> (Vec<f64>, u32) {
        let mut mono = Vec::new();
        let mut rate = 0;
        for (frames, format) in self.rx.try_iter() {
            rate = format.sample_rate;
            mono.extend(
                frames
                    .iter()
                    .step_by(usize::from(format.channels))
                    .map(|&s| f64::from(s)),
            );
        }
        (mono, rate)
    }
}

#[derive(Debug, Clone, Copy)]
struct Measurement {
    rms: f64,
    hz: f64,
    seconds: f64,
}

/// Captures `app` for a second (after a short settle time) and measures level and frequency of the first channel.
fn measure(source: &dyn AppAudioSource, app: &AudioApp) -> Measurement {
    let (handle, recording) = record(source, app);
    thread::sleep(Duration::from_millis(1300));
    assert!(handle.is_running());
    handle.stop();
    let (mono, rate) = recording.left();
    assert!(rate > 0, "no audio captured");
    let settle = (rate as usize * 3 / 10).min(mono.len());
    analyze(&mono[settle..], rate)
}

fn analyze(mono: &[f64], rate: u32) -> Measurement {
    let seconds = mono.len() as f64 / f64::from(rate);
    let rms = (mono.iter().map(|s| s * s).sum::<f64>() / mono.len().max(1) as f64).sqrt();
    let rising = mono.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    Measurement {
        rms,
        hz: rising as f64 / seconds.max(1e-9),
        seconds,
    }
}

/// At least `min_level` of the tone's full level, at the tone's frequency.
fn assert_tone(m: Measurement, min_level: f64) {
    assert!(m.seconds > 0.5, "only {:.2} s captured", m.seconds);
    assert!(m.rms > TONE_RMS * min_level, "level {:.3}", m.rms);
    assert!((m.hz - TONE_HZ).abs() < TONE_HZ * 0.05, "frequency {:.0} Hz", m.hz);
}

/// The RMS of the part of `mono` at `hz` (Goertzel).
fn level_at_frequency(mono: &[f64], rate: u32, hz: f64) -> f64 {
    let w = TAU * hz / f64::from(rate);
    let (mut s1, mut s2) = (0.0, 0.0);
    for &x in mono {
        let s = x + 2.0 * w.cos() * s1 - s2;
        (s2, s1) = (s1, s);
    }
    let power = s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2;
    power.sqrt() * std::f64::consts::SQRT_2 / mono.len().max(1) as f64
}

#[test]
#[ignore = "needs PipeWire"]
fn lists_captures_and_ducks_a_player() {
    let _turn = take_turn();
    let source = PipeWireSource::new().expect("PipeWire runs");
    let player = Player::start("RT Tone");
    // Our own process plays too; it isn't an app to pick.
    let own = InProcessPlayer::start("RT Own Output");
    own.open(440.0);

    let app = find_app(&source, "RT Tone");
    println!("listed: {app:?}");
    assert_eq!(app.pid, player.pid());
    assert_eq!(app.exe_path.as_deref(), Some(std::path::Path::new("/usr/bin/pw-cat")));
    wait_for(Duration::from_secs(5), || {
        (!volumes("RT Own Output").is_empty()).then_some(())
    })
    .expect("our own stream plays");
    let apps = source.list_apps().unwrap();
    assert!(
        apps.iter()
            .all(|a| a.pid != std::process::id() && a.name != "RT Own Output"),
        "our own stream is listed: {apps:?}"
    );
    drop(own);

    // Capturing the app gets the tone.
    let full = measure(&source, &app);
    println!("capture of the player: {full:?}");
    assert_tone(full, 0.9);

    // Ducking lowers the stream's volume and restores it exactly.
    let before = volumes("RT Tone");
    assert!(!before.is_empty(), "the player has a stream");
    source.set_app_volume(&app, 0.25).unwrap();
    let ducked_volumes = wait_for(Duration::from_secs(2), || {
        let now = volumes("RT Tone");
        close(&now, &scaled(&before, 0.25), 1e-4).then_some(now)
    });
    assert!(ducked_volumes.is_some(), "ducked to 25%: {:?}", volumes("RT Tone"));
    let ducked = measure(&source, &app);
    // Ducking again must not compound.
    source.set_app_volume(&app, 0.25).unwrap();
    thread::sleep(Duration::from_millis(200));
    let again = volumes("RT Tone");
    assert!(close(&again, &scaled(&before, 0.25), 1e-4), "ducked twice: {again:?}");
    source.set_app_volume(&app, 1.0).unwrap();
    let after = wait_for(Duration::from_secs(2), || {
        let now = volumes("RT Tone");
        (now == before).then_some(now)
    });
    assert_eq!(
        after,
        Some(before.clone()),
        "restored exactly: {:?}",
        volumes("RT Tone")
    );
    let restored = measure(&source, &app);

    // The capture is compensated, so ducking doesn't change its level.
    println!("ducked capture: {ducked:?}, restored: {restored:?}");
    assert!(
        (ducked.rms / full.rms - 1.0).abs() < 0.05,
        "ducked capture at {:.2}×",
        ducked.rms / full.rms
    );
    assert_tone(ducked, 0.9);
    assert_tone(restored, 0.9);

    // Around each step the compensation can be a cycle out; src/ramp.rs has what CI measured.
    let profile = duck_transitions(&source, &app, full.rms);
    report_profile(&profile);
}

#[test]
#[ignore = "needs PipeWire with pipewire-pulse, and pacat"]
fn a_pulseaudio_app_is_listed_captured_and_ducked_like_any_other() {
    let _turn = take_turn();
    let source = PipeWireSource::new().expect("PipeWire runs");
    let player = Player::start_pulse("RT Pulse Tone");
    let app = find_app(&source, "RT Pulse Tone");
    println!("listed: {app:?}");
    // The app's own process, not pipewire-pulse, which plays on its behalf.
    assert_eq!(app.pid, player.pid());
    assert!(
        app.exe_path.as_ref().is_some_and(|exe| exe.ends_with("pacat")),
        "{app:?}"
    );

    let full = measure(&source, &app);
    println!("capture of the player: {full:?}");
    assert_tone(full, 0.9);
    let before = volumes("RT Pulse Tone");
    let profile = duck_transitions(&source, &app, full.rms);
    report_profile(&profile);
    let after = wait_for(Duration::from_secs(2), || {
        (volumes("RT Pulse Tone") == before).then_some(())
    });
    assert!(after.is_some(), "restored: {:?}", volumes("RT Pulse Tone"));
}

/// Prints the 1 ms windows that are off and fails if any is more than [`MAX_OFF_DB`] off.
fn report_profile(profile: &[f64]) {
    let mut off: Vec<(usize, usize, f64, f64)> = Vec::new();
    for (ms, &ratio) in profile.iter().enumerate() {
        if (ratio - 1.0).abs() > 0.04 {
            match off.last_mut() {
                Some((start, len, lo, hi)) if *start + *len == ms => {
                    *len += 1;
                    *lo = ratio.min(*lo);
                    *hi = ratio.max(*hi);
                }
                _ => off.push((ms, 1, ratio, ratio)),
            }
        }
    }
    println!("1 ms windows off by more than 4% (start ms, length ms, min×, max×): {off:.2?}");
    let lo = profile.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = profile.iter().copied().fold(0.0, f64::max);
    let db = |ratio: f64| 20.0 * ratio.log10();
    println!(
        "worst 1 ms windows across {} ms of ducking and restoring: {lo:.3}× ({:+.2} dB), {hi:.3}× ({:+.2} dB)",
        profile.len(),
        db(lo),
        db(hi)
    );
    assert!(
        db(lo) >= -MAX_OFF_DB && db(hi) <= MAX_OFF_DB,
        "a 1 ms window is off by more than {MAX_OFF_DB} dB: {off:.2?}"
    );
}

/// Captures continuously while ducking to 25% and back [`FADES`] times, and returns the RMS of each 1 ms window (one
/// period of the tone) relative to `full_rms`, the level of an undisturbed capture.
fn duck_transitions(source: &dyn AppAudioSource, app: &AudioApp, full_rms: f64) -> Vec<f64> {
    let (handle, recording) = record(source, app);
    thread::sleep(Duration::from_millis(500));
    for _ in 0..FADES {
        source.set_app_volume(app, 0.25).unwrap();
        thread::sleep(Duration::from_millis(300));
        source.set_app_volume(app, 1.0).unwrap();
        thread::sleep(Duration::from_millis(300));
    }
    handle.stop();
    let (mono, rate) = recording.left();
    let mono = &mono[(rate as usize * 3 / 10).min(mono.len())..];
    mono.chunks_exact(rate as usize / 1000)
        .map(|w| (w.iter().map(|s| s * s).sum::<f64>() / w.len() as f64).sqrt() / full_rms)
        .collect()
}

#[test]
#[ignore = "needs PipeWire"]
fn a_stream_the_app_opens_while_ducked_is_ducked_and_captured() {
    let _turn = take_turn();
    let source = PipeWireSource::new().expect("PipeWire runs");
    let player = PlayerProcess::start("RT Two Streams");
    player.open(TONE_HZ);
    let app = find_app(&source, "RT Two Streams");
    assert_eq!(app.pid, player.pid);
    let (handle, recording) = record(&source, &app);
    source.set_app_volume(&app, 0.25).unwrap();
    let _ = recording.left();

    // A second stream, a new call say, at another pitch.
    player.open(440.0);
    let both_ducked = wait_for(Duration::from_secs(3), || {
        let now = volumes("RT Two Streams");
        (now.len() == 2 && close(&now, &[vec![0.25; 2], vec![0.25; 2]], 1e-4)).then_some(now)
    });
    assert!(
        both_ducked.is_some(),
        "the new stream is ducked: {:?}",
        volumes("RT Two Streams")
    );
    thread::sleep(Duration::from_millis(1000));
    let _ = recording.left();
    thread::sleep(Duration::from_millis(1000));
    let (mono, rate) = recording.left();
    handle.stop();
    let first = level_at_frequency(&mono, rate, TONE_HZ);
    let second = level_at_frequency(&mono, rate, 440.0);
    println!("captured while ducked: {first:.3} RMS at 1 kHz, {second:.3} at 440 Hz");
    // Both at full level: captured and compensated.
    assert!(
        (first / TONE_RMS - 1.0).abs() < 0.1,
        "first stream at {:.2}×",
        first / TONE_RMS
    );
    assert!(
        (second / TONE_RMS - 1.0).abs() < 0.1,
        "second stream at {:.2}×",
        second / TONE_RMS
    );

    source.set_app_volume(&app, 1.0).unwrap();
    let restored = wait_for(Duration::from_secs(2), || {
        let now = volumes("RT Two Streams");
        close(&now, &[vec![1.0; 2], vec![1.0; 2]], 1e-4).then_some(now)
    });
    assert!(restored.is_some(), "both restored: {:?}", volumes("RT Two Streams"));
}

#[test]
#[ignore = "needs PipeWire"]
fn a_volume_changed_by_hand_during_a_duck_is_left_alone() {
    let _turn = take_turn();
    let source = PipeWireSource::new().expect("PipeWire runs");
    let _player = Player::start("RT Hand Tone");
    let app = find_app(&source, "RT Hand Tone");
    source.set_app_volume(&app, 0.25).unwrap();
    let node = nodes_where(&dump(), "application.name", "RT Hand Tone")[0];
    wait_for(Duration::from_secs(2), || {
        close(&volumes("RT Hand Tone"), &[vec![0.25; 2]], 1e-4).then_some(())
    })
    .expect("ducked");
    set_by_hand(node, 0.6);
    wait_for(Duration::from_secs(2), || {
        close(&volumes("RT Hand Tone"), &[vec![0.6; 2]], 1e-4).then_some(())
    })
    .expect("set by hand");
    source.set_app_volume(&app, 1.0).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(volumes("RT Hand Tone"), [vec![0.6; 2]], "the user's volume stays");
    set_by_hand(node, 1.0);
}

#[test]
#[ignore = "needs PipeWire and the probe example"]
fn the_duck_guard_restores_a_player_ducked_by_a_killed_process() {
    let _turn = take_turn();
    let journal = probe_journal();
    let _player = Player::start("RT Guard Tone");
    let source = PipeWireSource::new().expect("PipeWire runs");
    find_app(&source, "RT Guard Tone");
    let before = volumes("RT Guard Tone");

    // The probe ducks with a duck journal and guard, like the app, and is killed without a chance to restore anything
    // itself, like a crash.
    crash_while_ducking("RT Guard Tone", &before, &[]);
    let start = Instant::now();
    let restored = wait_for(Duration::from_secs(10), || {
        (volumes("RT Guard Tone") == before).then(|| start.elapsed())
    });
    println!(
        "duck guard: volume {before:?} restored {restored:?} after kill -9, journal left: {}",
        journal.exists()
    );
    let restored = restored.expect("the duck guard restores the player's volume");
    assert!(restored <= Duration::from_secs(2), "restored only after {restored:?}");
    wait_for(Duration::from_secs(2), || (!journal.exists()).then_some(()))
        .expect("the duck guard removes the journal once everything is restored");
}

/// The duck journal of the probe, removed.
fn probe_journal() -> PathBuf {
    let journal = env::temp_dir().join("repeat-that-probe-duck.journal");
    let _ = fs::remove_file(&journal);
    journal
}

/// Runs the probe (with `env`) to duck the app called `name` from `before` to 25%, which it holds far longer than the
/// test needs, and kills it once it has, like a crash. The probe ducks with a duck journal, and a guard unless it runs
/// as in a Flatpak.
fn crash_while_ducking(name: &str, before: &[Vec<f32>], env: &[(&str, &str)]) {
    let probe = env::current_exe()
        .ok()
        .and_then(|test| Some(test.parent()?.parent()?.join("examples").join("probe")))
        .unwrap_or_default();
    assert!(
        probe.exists(),
        "{} is missing; build it with `cargo build -p rt-capture --example probe`",
        probe.display()
    );
    let mut prober = Command::new(&probe)
        .args(["duck", name, "0.25", "60"])
        .envs(env.iter().copied())
        .stdout(Stdio::null())
        .spawn()
        .expect("start the probe");
    let ducked = wait_for(Duration::from_secs(10), || {
        close(&volumes(name), &scaled(before, 0.25), 1e-4).then_some(())
    });
    if ducked.is_none() {
        let _ = prober.kill();
        panic!("the probe ducks the player to 25%: {:?}", volumes(name));
    }
    assert!(probe_journal_exists(), "the probe keeps a duck journal while ducked");
    prober.kill().unwrap();
    prober.wait().unwrap();
}

fn probe_journal_exists() -> bool {
    env::temp_dir().join("repeat-that-probe-duck.journal").exists()
}

#[test]
#[ignore = "needs PipeWire and the probe example"]
fn a_journal_without_a_guard_is_restored_once_the_app_plays_again() {
    let _turn = take_turn();
    let journal = probe_journal();
    let player = Player::start("RT Journal Tone");
    // The probe runs as in a Flatpak, which has no guard, and dies while ducking; then the app quits too. WirePlumber
    // remembers the ducked volume.
    crash_while_ducking(
        "RT Journal Tone",
        &[vec![1.0; 2]],
        &[("FLATPAK_ID", "me.roboroads.RepeatThat")],
    );
    thread::sleep(Duration::from_millis(500));
    assert!(
        close(&volumes("RT Journal Tone"), &[vec![0.25; 2]], 1e-4),
        "no guard restores it: {:?}",
        volumes("RT Journal Tone")
    );
    drop(player);
    assert!(journal.exists(), "the journal outlives the probe");

    let source = PipeWireSource::new().unwrap().with_duck_journal(journal.clone());
    // The app plays again: WirePlumber gives it the ducked volume, and the journal takes it back.
    let _player = Player::start("RT Journal Tone");
    let restored = wait_for(Duration::from_secs(5), || {
        close(&volumes("RT Journal Tone"), &[vec![1.0; 2]], 1e-4).then_some(())
    });
    println!("volume after the restart: {:?}", volumes("RT Journal Tone"));
    assert!(
        restored.is_some(),
        "restored from the journal: {:?}",
        volumes("RT Journal Tone")
    );
    wait_for(Duration::from_secs(2), || (!journal.exists()).then_some(())).expect("the journal is done");
    drop(source);
}

#[test]
#[ignore = "needs PipeWire"]
fn a_microphone_is_never_captured() {
    let _turn = take_turn();
    let source = PipeWireSource::new().expect("PipeWire runs");
    // A microphone (an `Audio/Source`, the default one, as the only one), with a 440 Hz tone on it.
    let mic = VirtualMic::create("rt-test-mic");
    let _speaker = Player::start_with("RT Mic Voice", 440.0, &["--target", "rt-test-mic"]);
    let _player = Player::start("RT Mic Tone");
    let app = find_app(&source, "RT Mic Tone");
    assert!(
        source
            .list_apps()
            .unwrap()
            .iter()
            .all(|a| !a.name.contains("rt-test-mic")),
        "a source isn't an app"
    );
    let (handle, recording) = record(&source, &app);
    thread::sleep(Duration::from_millis(1500));
    let (mono, rate) = recording.left();
    // Every link into our capture comes from the app's stream.
    let dump = dump();
    let ours: BTreeSet<u64> = dump
        .iter()
        .filter(|o| is(o, ":Node"))
        .filter(|o| {
            props(o)["node.name"]
                .as_str()
                .is_some_and(|n| n.starts_with("repeat-that-capture-"))
        })
        .filter_map(|o| o["id"].as_u64())
        .collect();
    let app_nodes = nodes_where(&dump, "application.name", "RT Mic Tone");
    let into_ours: Vec<u64> = dump
        .iter()
        .filter(|o| is(o, ":Link"))
        .filter(|o| o["info"]["input-node-id"].as_u64().is_some_and(|n| ours.contains(&n)))
        .filter_map(|o| o["info"]["output-node-id"].as_u64())
        .collect();
    handle.stop();
    println!(
        "links into our capture {ours:?} from {into_ours:?}; the app is {app_nodes:?}, the mic {}",
        mic.id
    );
    assert!(!into_ours.is_empty(), "the app is linked");
    assert!(
        into_ours.iter().all(|n| app_nodes.contains(n)),
        "only the app is linked"
    );
    let mono = &mono[(rate as usize / 2).min(mono.len())..];
    let (tone, voice) = (
        level_at_frequency(mono, rate, TONE_HZ),
        level_at_frequency(mono, rate, 440.0),
    );
    println!("captured {tone:.3} RMS at 1 kHz, {voice:.5} at 440 Hz");
    assert!(tone > TONE_RMS * 0.9, "the app is captured");
    assert!(voice < 0.001, "the microphone is not captured");
}

#[test]
#[ignore = "needs PipeWire"]
fn a_capture_ends_when_the_player_quits() {
    let _turn = take_turn();
    let source = PipeWireSource::new().expect("PipeWire runs");
    let player = Player::start("RT Quitting Tone");
    let app = find_app(&source, "RT Quitting Tone");
    let (handle, _recording) = record(&source, &app);
    thread::sleep(Duration::from_millis(300));
    assert!(handle.is_running());
    drop(player);
    let ended = wait_for(Duration::from_secs(3), || (!handle.is_running()).then_some(()));
    assert!(ended.is_some(), "the capture ends with the player");
}

/// A virtual microphone: a null `Audio/Source/Virtual` node, removed when dropped.
struct VirtualMic {
    id: u64,
}

impl VirtualMic {
    fn create(name: &str) -> Self {
        let status = Command::new("pw-cli")
            .args(["create-node", "adapter"])
            .arg(format!(
                "{{ factory.name = support.null-audio-sink node.name = {name} media.class = Audio/Source/Virtual \
                 audio.position = [ FL FR ] object.linger = true }}"
            ))
            .stdout(Stdio::null())
            .status()
            .expect("run pw-cli");
        assert!(status.success(), "pw-cli create-node: {status}");
        let id = wait_for(Duration::from_secs(5), || {
            nodes_where(&dump(), "node.name", name).first().copied()
        })
        .expect("the microphone appears");
        Self { id }
    }
}

impl Drop for VirtualMic {
    fn drop(&mut self) {
        let _ = Command::new("pw-cli")
            .args(["destroy", &self.id.to_string()])
            .stdout(Stdio::null())
            .status();
    }
}

/// A process of its own that plays any number of streams, opened one by one: an app with a new call, say. It is this
/// test binary again, running [`player_process`], detached (`setsid -f`) so it isn't in our process tree, which isn't
/// listed. Killed when dropped.
struct PlayerProcess {
    pid: u32,
    stdin: RefCell<ChildStdin>,
    _child: Child,
}

impl PlayerProcess {
    fn start(name: &str) -> Self {
        let mut child = Command::new("setsid")
            .arg("-f")
            .arg(env::current_exe().unwrap())
            .args(["--ignored", "--exact", "player_process", "--nocapture"])
            .env("RT_TEST_PLAYER", name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("run setsid (util-linux)");
        let stdin = RefCell::new(child.stdin.take().unwrap());
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let pid = lines
            .find_map(|line| line.ok()?.strip_prefix("player pid ")?.parse().ok())
            .expect("the player says its PID");
        // The rest of its output goes nowhere.
        thread::spawn(move || lines.for_each(drop));
        Self {
            pid,
            stdin,
            _child: child,
        }
    }

    fn open(&self, hz: f64) {
        writeln!(self.stdin.borrow_mut(), "{hz}").expect("the player runs");
    }
}

impl Drop for PlayerProcess {
    fn drop(&mut self) {
        unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
    }
}

/// The player process of [`PlayerProcess`]; does nothing in a normal test run.
#[test]
#[ignore = "run by the tests that need a player with several streams"]
fn player_process() {
    let Ok(name) = env::var("RT_TEST_PLAYER") else { return };
    println!("player pid {}", std::process::id());
    let player = InProcessPlayer::start(&name);
    for line in std::io::stdin().lines() {
        match line.ok().and_then(|l| l.trim().parse().ok()) {
            Some(hz) => player.open(hz),
            None => break,
        }
    }
}

/// Plays streams of sines from this process, on a PipeWire thread of its own. Ends when dropped.
struct InProcessPlayer {
    commands: pw::channel::Sender<f64>,
    thread: Option<JoinHandle<()>>,
}

impl InProcessPlayer {
    fn start(name: &str) -> Self {
        let (commands, receiver) = pw::channel::channel();
        let (ready, started) = mpsc::channel();
        let name = name.to_owned();
        let thread = thread::spawn(move || play(&name, receiver, ready));
        started.recv().expect("the player starts");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Opens another stream, playing `hz`.
    fn open(&self, hz: f64) {
        let _ = self.commands.send(hz);
    }
}

impl Drop for InProcessPlayer {
    fn drop(&mut self) {
        let _ = self.commands.send(0.0);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The PipeWire thread of an [`InProcessPlayer`]: opens a stream for every frequency it gets, and ends at 0.
fn play(name: &str, commands: pw::channel::Receiver<f64>, ready: mpsc::Sender<()>) {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).unwrap();
    let context = pw::context::ContextRc::new(&mainloop, None).unwrap();
    let core = context.connect_rc(None).unwrap();
    let streams = Rc::new(RefCell::new(Vec::new()));
    let open = {
        let (core, streams, name) = (core.clone(), Rc::clone(&streams), name.to_owned());
        move |hz: f64| {
            let mut props = pw::properties::PropertiesBox::new();
            props.insert("media.type", "Audio");
            props.insert("media.category", "Playback");
            props.insert("media.role", role(&name));
            props.insert("application.name", name.as_str());
            props.insert("node.name", format!("{}-{hz}", role(&name)));
            let stream = pw::stream::StreamRc::new(core.clone(), "rt-test-player", props).unwrap();
            let mut phase = 0.0f64;
            let listener = stream
                .add_local_listener_with_user_data(())
                .process(move |stream, _| {
                    let Some(mut buffer) = stream.dequeue_buffer() else {
                        return;
                    };
                    let requested = buffer.requested() as usize;
                    let data = &mut buffer.datas_mut()[0];
                    let Some(bytes) = data.data() else { return };
                    let frames = (bytes.len() / 8).min(if requested > 0 { requested } else { 1024 });
                    for frame in bytes[..frames * 8].as_chunks_mut::<8>().0 {
                        let sample = (0.5 * phase.sin()) as f32;
                        phase = (phase + TAU * hz / f64::from(RATE)) % TAU;
                        frame[..4].copy_from_slice(&sample.to_le_bytes());
                        frame[4..].copy_from_slice(&sample.to_le_bytes());
                    }
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = 8;
                    *chunk.size_mut() = (frames * 8) as u32;
                })
                .register()
                .unwrap();
            let mut info = pw::spa::param::audio::AudioInfoRaw::new();
            info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
            info.set_rate(RATE);
            info.set_channels(2);
            let mut position = [0; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
            position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
            position[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
            info.set_position(position);
            let format = pw::spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &pw::spa::pod::Value::Object(pw::spa::pod::Object {
                    type_: pw::spa::sys::SPA_TYPE_OBJECT_Format,
                    id: pw::spa::sys::SPA_PARAM_EnumFormat,
                    properties: info.into(),
                }),
            )
            .unwrap()
            .0
            .into_inner();
            stream
                .connect(
                    pw::spa::utils::Direction::Output,
                    None,
                    pw::stream::StreamFlags::AUTOCONNECT
                        | pw::stream::StreamFlags::MAP_BUFFERS
                        | pw::stream::StreamFlags::RT_PROCESS,
                    &mut [pw::spa::pod::Pod::from_bytes(&format).unwrap()],
                )
                .unwrap();
            streams.borrow_mut().push((listener, stream));
        }
    };
    let _commands = commands.attach(mainloop.loop_(), {
        let mainloop = mainloop.downgrade();
        move |hz| {
            if hz > 0.0 {
                open(hz);
            } else if let Some(mainloop) = mainloop.upgrade() {
                mainloop.quit();
            }
        }
    });
    let _ = ready.send(());
    mainloop.run();
    for (listener, stream) in streams.borrow_mut().drain(..) {
        let _ = stream.disconnect();
        drop(listener);
    }
}
