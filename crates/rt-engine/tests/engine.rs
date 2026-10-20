//! The engine end to end, with fake apps and an in-memory output: capture, replay, ducking, catch-up, reconnecting and
//! wipes. Everything stays in memory; the only file read is the rt-core speech fixture.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rt_capture::{AppAudioSource, AudioApp, CaptureError, CaptureFormat, CaptureHandle, FrameSink, MockSource};
use rt_core::{EnergyVad, Vad};
use rt_engine::{
    CaptureState, Engine, EngineConfig, EngineHandle, PlaybackState, Sensitivity, Status, TargetApp, TestOutput,
};

const RATE: u32 = 48_000;
const STEREO: CaptureFormat = CaptureFormat {
    sample_rate: RATE,
    channels: 2,
};
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../rt-core/tests/fixtures/speech_8s.wav");
/// The gap the player puts after each utterance.
const GAP: f32 = 0.25;

// --- A fake app ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Start(u32),
    Volume(u32, f32),
}

/// What [`FakeSource::fail_start`] makes `start` return.
#[derive(Debug, Clone, Copy)]
enum Failure {
    Backend,
    Unsupported,
}

/// Fake apps that play what the test gives them, with a switchable app list, and a record of every call.
///
/// A capture delivers the queued audio in 10 ms chunks, faster than real time ([`FakeSource::play`]) or in real time
/// ([`FakeSource::play_live`]), and silence in real time when there's nothing queued, like a quiet call.
struct FakeSource {
    inner: Arc<FakeInner>,
}

#[derive(Default)]
struct FakeInner {
    apps: Mutex<Vec<AudioApp>>,
    format: Mutex<Option<CaptureFormat>>,
    script: Mutex<VecDeque<Segment>>,
    calls: Mutex<Vec<(Instant, Call)>>,
    failure: Mutex<Option<Failure>>,
    /// Volume changes fail while set (they're still recorded).
    volume_fails: AtomicBool,
    captures: Mutex<Vec<Arc<AtomicBool>>>,
    /// How long stopping a capture takes.
    stop_delay: Mutex<Duration>,
}

struct Segment {
    samples: Vec<f32>,
    pos: usize,
    fast: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeSource {
    fn new(apps: Vec<AudioApp>) -> Arc<Self> {
        let inner = FakeInner::default();
        *lock(&inner.apps) = apps;
        Arc::new(Self { inner: Arc::new(inner) })
    }

    fn set_apps(&self, apps: Vec<AudioApp>) {
        *lock(&self.inner.apps) = apps;
    }

    /// The format of captures started from now on.
    fn set_format(&self, format: CaptureFormat) {
        *lock(&self.inner.format) = Some(format);
    }

    fn play(&self, samples: &[f32]) {
        self.queue(samples, true);
    }

    fn play_live(&self, samples: &[f32]) {
        self.queue(samples, false);
    }

    fn queue(&self, samples: &[f32], fast: bool) {
        lock(&self.inner.script).push_back(Segment {
            samples: samples.to_vec(),
            pos: 0,
            fast,
        });
    }

    /// Waits until everything queued was delivered and the worker took it.
    fn wait_played(&self) {
        wait_until(Duration::from_secs(30), "the fake app played its audio", || {
            lock(&self.inner.script).is_empty()
        });
        thread::sleep(Duration::from_millis(60));
    }

    /// Ends every running capture, as if the app quit.
    fn end_captures(&self) {
        for capture in lock(&self.inner.captures).drain(..) {
            capture.store(true, Ordering::Relaxed);
        }
    }

    /// Stopping a capture takes `delay` from now on, like WASAPI's.
    fn slow_stop(&self, delay: Duration) {
        *lock(&self.inner.stop_delay) = delay;
    }

    fn fail_volume(&self, fail: bool) {
        self.inner.volume_fails.store(fail, Ordering::Relaxed);
    }

    fn fail_start(&self, failure: Option<Failure>) {
        *lock(&self.inner.failure) = failure;
    }

    fn calls(&self) -> Vec<(Instant, Call)> {
        lock(&self.inner.calls).clone()
    }

    fn starts(&self) -> Vec<u32> {
        self.calls()
            .into_iter()
            .filter_map(|(_, c)| match c {
                Call::Start(pid) => Some(pid),
                Call::Volume(..) => None,
            })
            .collect()
    }

    /// Every volume change, with when it was asked for.
    fn volumes(&self) -> Vec<(Instant, u32, f32)> {
        self.calls()
            .into_iter()
            .filter_map(|(at, c)| match c {
                Call::Volume(pid, level) => Some((at, pid, level)),
                Call::Start(_) => None,
            })
            .collect()
    }

    fn levels(&self) -> Vec<f32> {
        self.volumes().into_iter().map(|(_, _, level)| level).collect()
    }

    fn record(&self, call: Call) {
        lock(&self.inner.calls).push((Instant::now(), call));
    }
}

impl FakeInner {
    /// Fills `out` from the script, up to `n` samples. Returns whether the audio taken is to be delivered fast, or
    /// `None` if nothing was queued.
    fn take(&self, n: usize, out: &mut Vec<f32>) -> Option<bool> {
        let mut script = lock(&self.script);
        let fast = script.front()?.fast;
        while out.len() < n {
            let Some(segment) = script.front_mut() else { break };
            let end = (segment.pos + n - out.len()).min(segment.samples.len());
            out.extend_from_slice(&segment.samples[segment.pos..end]);
            segment.pos = end;
            if segment.pos == segment.samples.len() {
                script.pop_front();
            }
        }
        Some(fast)
    }
}

impl AppAudioSource for FakeSource {
    fn list_apps(&self) -> rt_capture::Result<Vec<AudioApp>> {
        Ok(lock(&self.inner.apps).clone())
    }

    fn start(&self, app: &AudioApp, mut sink: FrameSink) -> rt_capture::Result<Box<dyn CaptureHandle>> {
        self.record(Call::Start(app.pid));
        match *lock(&self.inner.failure) {
            Some(Failure::Backend) => return Err(CaptureError::Backend("the fake failed to start".into())),
            Some(Failure::Unsupported) => return Err(CaptureError::Unsupported("the fake can't capture".into())),
            None => {}
        }
        if !lock(&self.inner.apps).iter().any(|a| a.pid == app.pid) {
            return Err(CaptureError::AppNotFound(app.pid));
        }
        let format = lock(&self.inner.format).unwrap_or(STEREO);
        let stop = Arc::new(AtomicBool::new(false));
        lock(&self.inner.captures).push(Arc::clone(&stop));
        let inner = Arc::clone(&self.inner);
        let stopped = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let chunk = (format.sample_rate / 100) as usize * usize::from(format.channels);
            let mut samples = Vec::with_capacity(chunk);
            let mut next = Instant::now();
            while !stopped.load(Ordering::Relaxed) {
                samples.clear();
                let fast = inner.take(chunk, &mut samples);
                samples.resize(chunk, 0.0);
                sink(&samples, format);
                if fast == Some(true) {
                    // 20 times real time.
                    thread::sleep(Duration::from_micros(500));
                    next = Instant::now();
                } else {
                    next += Duration::from_millis(10);
                    thread::sleep(next.saturating_duration_since(Instant::now()));
                }
            }
        });
        Ok(Box::new(FakeCapture {
            stop,
            thread: Some(thread),
            inner: Arc::clone(&self.inner),
        }))
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> rt_capture::Result<()> {
        self.record(Call::Volume(app.pid, level));
        if self.inner.volume_fails.load(Ordering::Relaxed) {
            return Err(CaptureError::Backend("the fake failed to change the volume".into()));
        }
        Ok(())
    }
}

struct FakeCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    inner: Arc<FakeInner>,
}

impl CaptureHandle for FakeCapture {
    fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    fn stop(self: Box<Self>) {}
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        let delay = *lock(&self.inner.stop_delay);
        thread::sleep(delay);
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The mock app, listed until its capture starts: it plays the fixture once and is gone, so it isn't captured again.
struct OnceSource {
    mock: MockSource,
    started: AtomicBool,
}

impl AppAudioSource for OnceSource {
    fn list_apps(&self) -> rt_capture::Result<Vec<AudioApp>> {
        if self.started.load(Ordering::Relaxed) {
            Ok(Vec::new())
        } else {
            self.mock.list_apps()
        }
    }

    fn start(&self, app: &AudioApp, sink: FrameSink) -> rt_capture::Result<Box<dyn CaptureHandle>> {
        self.started.store(true, Ordering::Relaxed);
        self.mock.start(app, sink)
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> rt_capture::Result<()> {
        self.mock.set_app_volume(app, level)
    }
}

// --- Helpers ------------------------------------------------------------------------------------------------------

fn app(pid: u32, name: &str) -> AudioApp {
    AudioApp {
        pid,
        name: name.into(),
        exe_path: Some(PathBuf::from(format!("/apps/{name}"))),
    }
}

fn discord(pid: u32) -> AudioApp {
    app(pid, "Discord")
}

/// A 220 Hz tone, the energy VAD's idea of speech, in `format`.
fn tone_in(format: CaptureFormat, seconds: f32, amplitude: f32) -> Vec<f32> {
    let frames = (seconds * format.sample_rate as f32) as usize;
    (0..frames)
        .flat_map(|i| {
            let s = amplitude * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / format.sample_rate as f32).sin();
            std::iter::repeat_n(s, usize::from(format.channels))
        })
        .collect()
}

fn tone(seconds: f32) -> Vec<f32> {
    tone_in(STEREO, seconds, 0.5)
}

fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * RATE as f32) as usize * 2]
}

/// One utterance: `seconds` of speech and a pause that ends it.
fn sentence(seconds: f32) -> Vec<f32> {
    [tone(seconds), silence(1.0)].concat()
}

fn engine(source: &Arc<FakeSource>, output: &TestOutput, config: EngineConfig) -> Engine {
    Engine::with_vad(
        Arc::clone(source) as Arc<dyn AppAudioSource>,
        Box::new(output.clone()),
        config,
        Box::new(EnergyVad::default()),
    )
}

/// An engine capturing Discord (PID 1).
fn capturing(output: &TestOutput, config: EngineConfig) -> (Arc<FakeSource>, Engine, EngineHandle) {
    let source = FakeSource::new(vec![discord(1)]);
    let engine = engine(&source, output, config);
    let handle = engine.handle();
    handle.set_target(Some(TargetApp::from(&discord(1))));
    wait_for(&handle, secs(2.0), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    (source, engine, handle)
}

fn secs(seconds: f32) -> Duration {
    Duration::from_secs_f32(seconds)
}

fn wait_until(timeout: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < timeout,
            "timed out after {timeout:?} waiting for {what}"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

/// Waits until the status satisfies `done`, and returns it.
fn wait_for(handle: &EngineHandle, timeout: Duration, what: &str, done: impl Fn(&Status) -> bool) -> Status {
    let start = Instant::now();
    loop {
        let status = handle.status();
        if done(&status) {
            return status;
        }
        assert!(
            start.elapsed() < timeout,
            "timed out after {timeout:?} waiting for {what}; status: {status:?}"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn wait_idle(handle: &EngineHandle, timeout: Duration) -> Instant {
    wait_for(handle, timeout, "the replay to end", |s| {
        s.playback == PlaybackState::Idle
    });
    Instant::now()
}

fn wait_playing(handle: &EngineHandle) {
    wait_for(handle, secs(1.0), "a replay", |s| s.playback != PlaybackState::Idle);
}

/// Waits until `seconds` of speech are buffered, and for the status to catch up with the last of it.
fn wait_buffered(handle: &EngineHandle, seconds: f32) -> f32 {
    wait_for(handle, secs(10.0), &format!("{seconds} s buffered"), |s| {
        s.buffered >= secs(seconds)
    });
    thread::sleep(Duration::from_millis(600));
    handle.status().buffered.as_secs_f32()
}

fn output_secs(output: &TestOutput) -> f32 {
    output.len() as f32 / output.sample_rate() as f32
}

/// Seconds of output louder than the noise floor.
fn loud_secs(samples: &[f32], rate: u32) -> f32 {
    samples.iter().filter(|s| s.abs() > 0.02).count() as f32 / rate as f32
}

fn assert_near(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual:.3}, expected {expected:.3} ± {tolerance}"
    );
}

// --- Replay -------------------------------------------------------------------------------------------------------

#[test]
fn replays_the_fixture_speech() {
    let source = Arc::new(OnceSource {
        mock: MockSource::from_wav(FIXTURE).unwrap().realtime(false).looping(false),
        started: AtomicBool::new(false),
    });
    let output = TestOutput::new(RATE).speed(8.0);
    let engine = Engine::new(source, Box::new(output.clone()), EngineConfig::default());
    let handle = engine.handle();
    handle.set_target(Some(TargetApp {
        exe_path: None,
        name: "speech_8s".into(),
    }));

    // The fixture plays once; the capture ends with the last sentence closed and everything kept.
    let status = wait_for(&handle, secs(20.0), "the fixture captured", |s| {
        s.capture == CaptureState::Waiting && s.buffered > secs(1.0)
    });
    assert!(!status.vad_fallback, "Silero VAD loads");
    let buffered = status.buffered.as_secs_f32();
    assert!((5.0..8.5).contains(&buffered), "{buffered} s of speech in 8 s of audio");
    assert_eq!(handle.dropped_samples(), 0);

    // Replay, and step back to the oldest utterance: it plays through to the end of the fixture.
    handle.replay();
    for _ in 0..20 {
        handle.replay();
    }
    wait_playing(&handle);
    wait_idle(&handle, secs(10.0));
    let played = output_secs(&output);
    assert!(
        (buffered..buffered + 2.5).contains(&played),
        "played {played} s of {buffered} s buffered"
    );
    // All of the fixture's speech, upsampled from 16 kHz to 48 kHz.
    let energy = |samples: &[f32]| samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>();
    let fixture: Vec<f32> = hound::WavReader::open(FIXTURE)
        .unwrap()
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    let ratio = energy(&output.samples()) / 3.0 / energy(&fixture);
    assert!(
        (0.85..1.15).contains(&ratio),
        "the replay has {ratio:.2}x the fixture's energy"
    );
    assert_eq!(handle.status().capture, CaptureState::Waiting);
}

#[test]
fn reports_progress_during_a_replay() {
    let output = TestOutput::new(RATE);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(3.0));
    source.wait_played();
    wait_buffered(&handle, 3.2);

    let updates = handle.subscribe();
    let first = updates.recv().unwrap();
    assert_eq!(first.playback, PlaybackState::Idle);
    assert_eq!(first.behind, None);
    handle.replay();
    let start = Instant::now();
    let mut during = Vec::new();
    while start.elapsed() < secs(1.8) {
        if let Ok(status) = updates.recv_timeout(Duration::from_millis(50)) {
            during.push(status);
        }
    }
    let behind: Vec<f32> = during
        .iter()
        .filter(|s| s.playback == PlaybackState::Replaying)
        .filter_map(|s| s.behind)
        .map(|b| b.as_secs_f32())
        .collect();
    // About 3.6 s behind at the start, and an update every 500 ms.
    assert!(behind.len() >= 3, "{behind:?}");
    assert!(behind[0] > 3.0, "{behind:?}");
    assert!(behind.windows(2).all(|w| w[1] <= w[0]), "{behind:?}");
    assert_eq!(during.last().unwrap().output.device_name.as_deref(), Some("speakers"));
}

#[test]
fn ducks_while_replaying_and_restores_after() {
    let output = TestOutput::new(RATE).speed(4.0);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(1.0));
    source.wait_played();
    wait_buffered(&handle, 1.2);

    let start = Instant::now();
    handle.replay();
    wait_playing(&handle);
    let ended = wait_idle(&handle, secs(3.0));
    thread::sleep(Duration::from_millis(600));
    let volumes = source.volumes();
    assert_eq!(source.levels(), [0.2, 1.0]);
    assert!(volumes.iter().all(|&(_, pid, _)| pid == 1));
    let ducked = volumes[0].0 - start;
    assert!(ducked < Duration::from_millis(100), "ducked after {ducked:?}");
    let restored = volumes[1].0 - ended;
    assert!(
        (Duration::from_millis(280)..Duration::from_millis(600)).contains(&restored),
        "restored {restored:?} after the end"
    );

    // A replay within 300 ms of the end keeps the app ducked.
    handle.replay();
    wait_playing(&handle);
    wait_idle(&handle, secs(3.0));
    thread::sleep(Duration::from_millis(100));
    handle.replay();
    wait_playing(&handle);
    wait_idle(&handle, secs(3.0));
    thread::sleep(Duration::from_millis(600));
    assert_eq!(source.levels(), [0.2, 1.0, 0.2, 1.0]);
}

#[test]
fn a_failed_restore_is_retried() {
    let output = TestOutput::new(RATE).speed(4.0);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(1.0));
    source.wait_played();
    wait_buffered(&handle, 1.2);
    handle.replay();
    wait_until(secs(1.0), "ducked", || source.levels() == [0.2]);

    source.fail_volume(true);
    wait_idle(&handle, secs(3.0));
    wait_until(secs(1.0), "a restore", || source.levels().len() == 2);
    source.fail_volume(false);
    wait_until(secs(2.0), "the restore retried", || source.levels().len() == 3);
    assert_eq!(source.levels(), [0.2, 1.0, 1.0]);
    // Restored: no more tries.
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(source.levels().len(), 3);
}

#[test]
fn leaves_the_volume_alone_with_ducking_off() {
    let output = TestOutput::new(RATE).speed(4.0);
    let config = EngineConfig {
        duck: None,
        ..EngineConfig::default()
    };
    let (source, _engine, handle) = capturing(&output, config);
    source.play(&sentence(1.0));
    source.wait_played();
    wait_buffered(&handle, 1.2);
    handle.replay();
    wait_playing(&handle);
    wait_idle(&handle, secs(3.0));
    thread::sleep(Duration::from_millis(500));
    assert!(source.levels().is_empty(), "{:?}", source.levels());
    assert!(!output.is_empty());
}

#[test]
fn plays_through_to_live() {
    let output = TestOutput::new(RATE);
    let config = EngineConfig {
        replay_speed: 1.5,
        ..EngineConfig::default()
    };
    let (source, _engine, handle) = capturing(&output, config);
    // Someone has been talking for 1.6 s when the replay starts, and goes on for 4.4 s more.
    source.play_live(&[tone(6.0), silence(2.0)].concat());
    thread::sleep(secs(1.6));
    let start = Instant::now();
    handle.replay();
    wait_playing(&handle);

    // At 1.5x the replay reaches them after about 3.6 s, and follows them while they talk.
    thread::sleep(secs(4.2).saturating_sub(start.elapsed()));
    assert_ne!(handle.status().playback, PlaybackState::Idle, "still talking");
    wait_idle(&handle, secs(5.0));
    let took = start.elapsed().as_secs_f32();
    // They stop 4.4 s in; the pause closes the sentence 0.7 s later.
    assert!(took > 5.0, "the replay ended {took} s in, before they stopped talking");
    let loud = loud_secs(&output.samples(), RATE);
    assert!(loud > 3.9, "{loud} s of speech played");
}

#[test]
fn a_replay_started_while_the_beep_plays_out_goes_on() {
    // The beep plays slowly, so it's still playing out when the next replay starts, of a capture at another rate.
    let output = TestOutput::new(RATE).speed(0.1);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    handle.set_paused(true);
    wait_for(&handle, secs(1.0), "paused", |s| s.capture == CaptureState::Paused);
    handle.replay();
    wait_until(secs(1.0), "the beep", || output.open_streams() == 1);

    let mono = CaptureFormat {
        sample_rate: 16_000,
        channels: 1,
    };
    source.set_format(mono);
    handle.set_paused(false);
    wait_for(&handle, secs(1.0), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    source.play(&[tone_in(mono, 1.0, 0.5), vec![0.0; 16_000]].concat());
    source.wait_played();
    assert_eq!(output.open_streams(), 1, "the beep still plays");

    handle.replay();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(handle.status().playback, PlaybackState::Replaying);
    assert_eq!(source.levels(), [0.2]);
}

#[test]
fn beeps_when_there_is_nothing_to_replay() {
    let source = FakeSource::new(vec![]);
    let output = TestOutput::new(RATE);
    let engine = engine(&source, &output, EngineConfig::default());
    let handle = engine.handle();
    let updates = handle.subscribe();
    handle.replay();
    wait_until(secs(2.0), "the beep", || {
        !output.is_empty() && output.open_streams() == 0
    });
    let beep = output.samples();
    let seconds = beep.len() as f32 / RATE as f32;
    assert!((0.15..0.25).contains(&seconds), "a {seconds} s beep");
    assert!(loud_secs(&beep, RATE) > 0.1);
    assert!(updates.try_iter().all(|s| s.playback == PlaybackState::Idle));
    assert!(source.levels().is_empty(), "the beep doesn't duck");
}

// --- Catch-up and stop --------------------------------------------------------------------------------------------

#[test]
fn catch_up_from_idle_plays_faster() {
    let output = TestOutput::new(RATE).speed(2.0);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(3.0));
    source.wait_played();
    let buffered = wait_buffered(&handle, 3.2);

    handle.toggle_catch_up();
    wait_for(&handle, secs(1.0), "catching up", |s| {
        s.playback == PlaybackState::CatchingUp
    });
    wait_idle(&handle, secs(5.0));
    assert_near(output_secs(&output), (buffered + GAP) / 1.5, 0.12, "output at 1.5x");

    // Catch-up ends with the replay: the next one is at normal speed.
    output.clear();
    handle.replay();
    wait_for(&handle, secs(1.0), "replaying", |s| {
        s.playback == PlaybackState::Replaying
    });
    wait_idle(&handle, secs(5.0));
    assert_near(output_secs(&output), buffered + GAP, 0.12, "output at 1.0x");
}

#[test]
fn catch_up_applies_mid_replay() {
    let output = TestOutput::new(RATE).speed(2.0);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(3.0));
    source.wait_played();
    let total = wait_buffered(&handle, 3.2) + GAP;

    handle.replay();
    wait_until(secs(2.0), "a second played", || output_secs(&output) >= 1.0);
    handle.toggle_catch_up();
    let switched = output_secs(&output);
    wait_for(&handle, secs(1.0), "catching up", |s| {
        s.playback == PlaybackState::CatchingUp
    });
    wait_idle(&handle, secs(5.0));
    // What was queued for the output still plays at 1.0x.
    let expected = switched + 0.08 + (total - switched - 0.08) / 1.5;
    assert_near(output_secs(&output), expected, 0.15, "output with catch-up from 1 s on");

    // Toggled twice: back to replay speed.
    output.clear();
    handle.replay();
    wait_playing(&handle);
    handle.toggle_catch_up();
    wait_for(&handle, secs(1.0), "catching up", |s| {
        s.playback == PlaybackState::CatchingUp
    });
    handle.toggle_catch_up();
    wait_for(&handle, secs(1.0), "replaying", |s| {
        s.playback == PlaybackState::Replaying
    });
    wait_idle(&handle, secs(5.0));
    assert!(output_secs(&output) > total - 0.3, "{} s", output_secs(&output));
}

#[test]
fn stop_ends_at_once_and_restores_after() {
    let output = TestOutput::new(RATE);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(3.0));
    source.wait_played();
    wait_buffered(&handle, 3.2);

    handle.replay();
    wait_until(secs(2.0), "half a second played", || output_secs(&output) >= 0.5);
    let stopped = Instant::now();
    handle.stop();
    wait_idle(&handle, Duration::from_millis(100));
    thread::sleep(Duration::from_millis(100));
    let played = output.len();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(output.len(), played, "nothing plays after a stop");
    assert!((played as f32 / RATE as f32) < 0.75, "{played} samples");

    thread::sleep(Duration::from_millis(300));
    let volumes = source.volumes();
    assert_eq!(source.levels(), [0.2, 1.0]);
    let restored = volumes[1].0 - stopped;
    assert!(
        (Duration::from_millis(290)..Duration::from_millis(600)).contains(&restored),
        "restored {restored:?} after the stop"
    );
}

// --- Target app and reconnecting ----------------------------------------------------------------------------------

#[test]
fn waits_for_the_target_and_reconnects_when_it_restarts() {
    let source = FakeSource::new(vec![]);
    let output = TestOutput::new(RATE);
    let engine = engine(&source, &output, EngineConfig::default());
    let handle = engine.handle();
    handle.set_target(Some(TargetApp::from(&discord(1))));
    let status = wait_for(&handle, secs(1.0), "waiting", |s| s.capture == CaptureState::Waiting);
    assert_eq!(status.target_name.as_deref(), Some("Discord"));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(handle.status().capture, CaptureState::Waiting);

    // It shows up: captured within one poll.
    source.set_apps(vec![discord(1)]);
    wait_for(&handle, secs(2.5), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    source.play(&sentence(1.0));
    source.wait_played();
    let buffered = wait_buffered(&handle, 1.2);

    // It quits: waiting again, and what it said stays.
    source.set_apps(vec![]);
    source.end_captures();
    let status = wait_for(&handle, secs(1.0), "waiting", |s| s.capture == CaptureState::Waiting);
    assert_eq!(status.buffered.as_secs_f32(), buffered);

    // It restarts with a new PID: the same executable, so the same app.
    source.set_apps(vec![app(7, "Slack"), discord(2)]);
    wait_for(&handle, secs(2.5), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    assert_eq!(source.starts(), [1, 2]);
    assert_eq!(handle.status().buffered.as_secs_f32(), buffered);
}

#[test]
fn a_reconnect_during_a_replay_ducks_the_restarted_app() {
    let output = TestOutput::new(RATE);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(4.0));
    source.wait_played();
    wait_buffered(&handle, 4.2);
    handle.replay();
    wait_until(secs(1.0), "ducked", || source.levels() == [0.2]);

    // Discord restarts with a new PID while the replay plays.
    source.set_apps(vec![]);
    source.end_captures();
    wait_for(&handle, secs(1.0), "waiting", |s| s.capture == CaptureState::Waiting);
    source.set_apps(vec![discord(2)]);
    wait_for(&handle, secs(2.5), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    thread::sleep(Duration::from_millis(300));
    assert_ne!(handle.status().playback, PlaybackState::Idle, "still replaying");
    let volumes = || -> Vec<(u32, f32)> {
        source
            .volumes()
            .into_iter()
            .map(|(_, pid, level)| (pid, level))
            .collect()
    };
    assert!(
        volumes().contains(&(2, 0.2)),
        "the restarted app isn't ducked: {:?}",
        volumes()
    );

    wait_idle(&handle, secs(5.0));
    thread::sleep(Duration::from_millis(600));
    assert_eq!(volumes().last(), Some(&(2, 1.0)));
}

#[test]
fn matches_the_mock_app_by_name() {
    let source = Arc::new(MockSource::from_wav(FIXTURE).unwrap());
    let output = TestOutput::new(RATE);
    let engine = Engine::with_vad(
        source,
        Box::new(output.clone()),
        EngineConfig::default(),
        Box::new(EnergyVad::default()),
    );
    let handle = engine.handle();
    handle.set_target(Some(TargetApp {
        exe_path: None,
        name: "speech_8s".into(),
    }));
    wait_for(&handle, secs(1.0), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    assert_eq!(handle.list_apps().unwrap()[0].name, "speech_8s");
}

#[test]
fn retries_a_failed_start_but_not_an_unsupported_one() {
    let source = FakeSource::new(vec![discord(1)]);
    source.fail_start(Some(Failure::Backend));
    let output = TestOutput::new(RATE);
    let engine = engine(&source, &output, EngineConfig::default());
    let handle = engine.handle();
    handle.set_target(Some(TargetApp::from(&discord(1))));
    let status = wait_for(&handle, secs(1.0), "an error", |s| {
        matches!(s.capture, CaptureState::Error(_))
    });
    let CaptureState::Error(message) = status.capture else {
        unreachable!()
    };
    assert!(message.contains("the fake failed to start"), "{message}");
    source.fail_start(None);
    wait_for(&handle, secs(2.5), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    assert_eq!(source.starts(), [1, 1]);

    let source = FakeSource::new(vec![discord(1)]);
    source.fail_start(Some(Failure::Unsupported));
    let engine = self::engine(&source, &output, EngineConfig::default());
    let handle = engine.handle();
    handle.set_target(Some(TargetApp::from(&discord(1))));
    wait_for(&handle, secs(1.0), "unsupported", |s| {
        matches!(s.capture, CaptureState::Unsupported(_))
    });
    thread::sleep(secs(2.5));
    assert_eq!(source.starts(), [1]);
}

// --- Wipes --------------------------------------------------------------------------------------------------------

#[test]
fn pause_wipes_the_buffer_and_resume_captures_again() {
    let output = TestOutput::new(RATE).speed(4.0);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(1.0));
    source.wait_played();
    wait_buffered(&handle, 1.2);

    handle.set_paused(true);
    let status = wait_for(&handle, secs(1.0), "paused and wiped", |s| {
        s.capture == CaptureState::Paused && s.buffered.is_zero()
    });
    assert_eq!(status.target_name.as_deref(), Some("Discord"));
    // Nothing left to replay: the beep.
    handle.replay();
    wait_until(secs(2.0), "the beep", || {
        !output.is_empty() && output.open_streams() == 0
    });
    assert!(output_secs(&output) < 0.25);

    handle.set_paused(false);
    wait_for(&handle, secs(1.0), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    assert_eq!(source.starts(), [1, 1]);
}

#[test]
fn a_replay_pressed_while_the_capture_stops_for_a_pause_finds_nothing() {
    let output = TestOutput::new(RATE).speed(4.0);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(1.0));
    source.wait_played();
    wait_buffered(&handle, 1.2);

    source.slow_stop(Duration::from_millis(400));
    handle.set_paused(true);
    thread::sleep(Duration::from_millis(100));
    handle.replay();
    wait_until(secs(3.0), "the beep", || {
        !output.is_empty() && output.open_streams() == 0
    });
    // The beep peaks at -18 dBFS, the speech at 0.5.
    let peak = output.samples().iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak < 0.2, "the wiped speech played (peak {peak})");
}

#[test]
fn another_target_stops_the_replay_restores_the_volume_and_wipes() {
    let output = TestOutput::new(RATE);
    let source = FakeSource::new(vec![discord(1), app(2, "Teams")]);
    let engine = engine(&source, &output, EngineConfig::default());
    let handle = engine.handle();
    handle.set_target(Some(TargetApp::from(&discord(1))));
    wait_for(&handle, secs(1.0), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });
    source.play(&sentence(3.0));
    source.wait_played();
    wait_buffered(&handle, 3.2);
    handle.replay();
    wait_until(secs(1.0), "ducked", || source.levels() == [0.2]);
    wait_until(secs(1.0), "some of it played", || output_secs(&output) >= 0.3);

    handle.set_target(Some(TargetApp::from(&app(2, "Teams"))));
    let status = wait_for(&handle, secs(1.0), "Teams, wiped", |s| {
        s.target_name.as_deref() == Some("Teams") && s.buffered.is_zero() && s.capture == CaptureState::Capturing
    });
    assert_eq!(status.playback, PlaybackState::Idle);
    // The old app got its volume back; the new one was never ducked.
    let volumes: Vec<(u32, f32)> = source
        .volumes()
        .into_iter()
        .map(|(_, pid, level)| (pid, level))
        .collect();
    assert_eq!(volumes, [(1, 0.2), (1, 1.0)]);
    assert_eq!(source.starts(), [1, 2]);
    // It fades out within a few milliseconds.
    thread::sleep(Duration::from_millis(50));
    let played = output.len();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(output.len(), played, "the replay stopped");
}

#[test]
fn dropping_the_engine_wipes_and_restores_the_volume() {
    let output = TestOutput::new(RATE);
    let (source, engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(3.0));
    source.wait_played();
    wait_buffered(&handle, 3.2);
    handle.replay();
    wait_until(secs(1.0), "ducked", || source.levels() == [0.2]);

    drop(engine);
    assert_eq!(source.levels(), [0.2, 1.0]);
    assert!(handle.status().buffered.is_zero());
    assert_eq!(output.open_streams(), 0);
    // A handle outliving the engine does nothing.
    handle.replay();
    handle.set_paused(true);
}

// --- Output -------------------------------------------------------------------------------------------------------

#[test]
fn falls_back_to_the_default_device() {
    let output = TestOutput::new(RATE)
        .speed(4.0)
        .with_devices(&["speakers", "headphones"]);
    output.fail_device("headphones");
    let config = EngineConfig {
        output_device: Some("headphones".into()),
        ..EngineConfig::default()
    };
    let (source, _engine, handle) = capturing(&output, config.clone());
    source.play(&sentence(1.0));
    source.wait_played();
    wait_buffered(&handle, 1.2);

    handle.replay();
    let status = wait_for(&handle, secs(1.0), "a replay", |s| {
        s.playback == PlaybackState::Replaying
    });
    assert_eq!(status.output.device_name.as_deref(), Some("speakers"));
    assert!(status.output.fell_back);
    assert_eq!(status.output.error, None);
    assert_eq!(output.opens(), [Some("headphones".into()), None]);
    wait_idle(&handle, secs(3.0));

    // A device that isn't there (unplugged since it was chosen) falls back the same way.
    handle.set_config(EngineConfig {
        output_device: Some("usb".into()),
        ..config
    });
    handle.replay();
    let status = wait_for(&handle, secs(1.0), "a replay", |s| {
        s.playback == PlaybackState::Replaying
    });
    assert!(status.output.fell_back);
    wait_idle(&handle, secs(3.0));
}

#[test]
fn reopens_the_default_device_when_the_stream_fails() {
    let output = TestOutput::new(RATE).with_devices(&["speakers", "headphones"]);
    let config = EngineConfig {
        output_device: Some("headphones".into()),
        ..EngineConfig::default()
    };
    let (source, _engine, handle) = capturing(&output, config);
    source.play(&sentence(2.0));
    source.wait_played();
    let total = wait_buffered(&handle, 2.2) + GAP;

    handle.replay();
    wait_until(secs(2.0), "half a second played", || output_secs(&output) >= 0.5);
    assert_eq!(handle.status().output.device_name.as_deref(), Some("headphones"));
    // Unplugged mid-replay.
    output.break_streams();
    let status = wait_for(&handle, secs(1.0), "reopened", |s| s.output.fell_back);
    assert_eq!(status.output.device_name.as_deref(), Some("speakers"));
    assert_eq!(status.playback, PlaybackState::Replaying);
    wait_idle(&handle, secs(5.0));
    assert_eq!(output.opens(), [Some("headphones".into()), None]);
    // What was queued for the broken stream is lost; the rest plays.
    assert!(output_secs(&output) > total - 0.3, "{} s", output_secs(&output));
}

#[test]
fn without_any_device_the_replay_is_aborted_and_nothing_ducks() {
    let output = TestOutput::new(RATE);
    output.fail_all();
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(1.0));
    source.wait_played();
    wait_buffered(&handle, 1.2);

    handle.replay();
    let status = wait_for(&handle, secs(1.0), "an output error", |s| s.output.error.is_some());
    assert_eq!(status.playback, PlaybackState::Idle);
    thread::sleep(Duration::from_millis(500));
    assert!(source.levels().is_empty());
    assert_eq!(handle.status().playback, PlaybackState::Idle);
}

#[test]
fn lists_output_devices() {
    let output = TestOutput::new(RATE).with_devices(&["speakers", "headphones"]);
    let source = FakeSource::new(vec![]);
    let engine = engine(&source, &output, EngineConfig::default());
    let devices = engine.handle().list_output_devices().unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices[0].is_default);
}

// --- Capture format and live settings -----------------------------------------------------------------------------

#[test]
fn another_capture_format_rebuilds_the_recorder() {
    let output = TestOutput::new(RATE);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    source.play(&sentence(2.0));
    source.wait_played();
    wait_buffered(&handle, 2.2);

    // The app comes back with a device at 16 kHz mono.
    let mono = CaptureFormat {
        sample_rate: 16_000,
        channels: 1,
    };
    source.set_format(mono);
    source.end_captures();
    wait_for(&handle, secs(1.0), "waiting", |s| s.capture == CaptureState::Waiting);
    source.play(&[tone_in(mono, 0.5, 0.5), vec![0.0; 16_000]].concat());
    // Played once the app is captured again.
    source.wait_played();
    thread::sleep(Duration::from_millis(600));
    let status = handle.status();
    assert_eq!(status.capture, CaptureState::Capturing);
    // Half a second of speech and its padding after it; the capture started with it, so there's no padding before.
    assert_near(status.buffered.as_secs_f32(), 0.7, 0.1, "buffered");
}

#[test]
fn a_smaller_buffer_applies_at_once_without_a_wipe() {
    let output = TestOutput::new(RATE);
    let (source, _engine, handle) = capturing(&output, EngineConfig::default());
    for _ in 0..5 {
        source.play(&sentence(1.0));
    }
    source.wait_played();
    wait_buffered(&handle, 6.5);

    handle.set_config(EngineConfig {
        buffer: secs(3.0),
        ..EngineConfig::default()
    });
    let status = wait_for(&handle, Duration::from_millis(200), "the buffer shrinks", |s| {
        s.buffered <= secs(3.0)
    });
    assert!(status.buffered >= secs(2.5), "{:?}", status.buffered);
}

/// Reports a frame's level as its speech probability, so the threshold decides what counts as speech.
struct LevelVad;

impl Vad for LevelVad {
    fn frame_len(&self) -> usize {
        512
    }

    fn speech_probability(&mut self, frame: &[f32]) -> f32 {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        (rms * 2.0).min(1.0)
    }

    fn reset(&mut self) {}
}

#[test]
fn a_new_sensitivity_applies_live_without_a_wipe() {
    let source = FakeSource::new(vec![discord(1)]);
    let output = TestOutput::new(RATE);
    let engine = Engine::with_vad(
        Arc::clone(&source) as Arc<dyn AppAudioSource>,
        Box::new(output.clone()),
        EngineConfig::default(),
        Box::new(LevelVad),
    );
    let handle = engine.handle();
    handle.set_target(Some(TargetApp::from(&discord(1))));
    wait_for(&handle, secs(1.0), "capturing", |s| {
        s.capture == CaptureState::Capturing
    });

    // Loud speech counts at normal sensitivity (probability 0.71), quiet speech (0.42) doesn't.
    let quiet = [tone_in(STEREO, 1.0, 0.3), silence(1.0)].concat();
    source.play(&sentence(1.0));
    source.play(&quiet);
    source.wait_played();
    thread::sleep(Duration::from_millis(600));
    // A second of speech and up to 200 ms of padding on each side.
    let loud = handle.status().buffered.as_secs_f32();
    assert!((1.15..1.5).contains(&loud), "{loud} s of loud speech buffered");

    handle.set_config(EngineConfig {
        sensitivity: Sensitivity::High,
        ..EngineConfig::default()
    });
    source.play(&quiet);
    source.wait_played();
    let both = wait_buffered(&handle, loud + 1.2);
    assert_near(both - loud, 1.4, 0.1, "quiet speech buffered");
}
