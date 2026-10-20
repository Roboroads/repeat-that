//! Where replays play: [`CpalOutput`] (the sound card), [`NullOutput`] (real-time, discarded) and [`TestOutput`]
//! (collected in memory, for tests).
//!
//! The player thread renders mono audio at the device's rate into an [`OutputFeed`]; a stream pulls from it on its own
//! thread and copies it to every channel.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// An output device, as [`OutputBackend::devices`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDevice {
    /// Stable across runs, for the settings ([`EngineConfig::output_device`](crate::EngineConfig::output_device)).
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputError {
    /// There is no output device at all.
    NoDevice,
    /// The device with this id isn't there (any more).
    NotFound(String),
    /// The audio API failed.
    Backend(String),
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => f.write_str("no audio output device"),
            Self::NotFound(id) => write!(f, "output device {id} not found"),
            Self::Backend(msg) => write!(f, "audio output error: {msg}"),
        }
    }
}

impl std::error::Error for OutputError {}

/// Opens output streams. Shared between the engine's player thread and [`EngineHandle`](crate::EngineHandle).
pub trait OutputBackend: Send + Sync {
    /// Devices that can play right now.
    fn devices(&self) -> Result<Vec<OutputDevice>, OutputError>;

    /// Opens the device with `id` (`None`: the system default) and starts playing what comes into `feed`. The stream
    /// plays until it is dropped.
    fn open(&self, id: Option<&str>, feed: OutputFeed) -> Result<Box<dyn OutputStream>, OutputError>;
}

/// A playing output stream. Dropping it closes the device.
pub trait OutputStream {
    /// The rate the feed is played at.
    fn sample_rate(&self) -> u32;

    fn device_name(&self) -> &str;

    /// Set once the stream failed for good, e.g. because the device was unplugged. It plays nothing more.
    fn failure(&self) -> Option<String>;
}

/// Fade applied when a replay stops in the middle, in samples: about 5 ms.
const STOP_FADE: usize = 256;

/// Mono audio on its way to an output stream, filled by the player thread. Lock-free: safe to use in an audio callback.
pub struct OutputFeed {
    consumer: rtrb::Consumer<f32>,
    stopping: Arc<AtomicBool>,
    /// Samples left of the fade after a stop.
    fade_left: usize,
}

/// The player thread's end of an [`OutputFeed`].
pub(crate) struct FeedWriter {
    pub(crate) producer: rtrb::Producer<f32>,
    stopping: Arc<AtomicBool>,
}

impl FeedWriter {
    /// Samples waiting to be played.
    pub(crate) fn queued(&self) -> usize {
        self.producer.buffer().capacity() - self.producer.slots()
    }

    /// Writes as much of `samples` as fits, returns how much that was.
    pub(crate) fn write(&mut self, samples: &[f32]) -> usize {
        let (written, _) = self.producer.push_partial_slice(samples);
        written.len()
    }

    /// Fades out what is playing and drops the rest.
    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }
}

impl OutputFeed {
    /// A feed holding up to `capacity` samples.
    pub(crate) fn new(capacity: usize) -> (FeedWriter, Self) {
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        let stopping = Arc::new(AtomicBool::new(false));
        let writer = FeedWriter {
            producer,
            stopping: Arc::clone(&stopping),
        };
        let feed = Self {
            consumer,
            stopping,
            fade_left: STOP_FADE,
        };
        (writer, feed)
    }

    /// Takes up to `out.len()` samples, and returns how many it took. After a stop it fades out over a few
    /// milliseconds, then takes nothing more.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        if !self.stopping.load(Ordering::Relaxed) {
            return self.consumer.pop_partial_slice(out).0.len();
        }
        if self.fade_left == 0 {
            // Drop the rest, so nothing is left over in memory either.
            let n = self.consumer.slots();
            if let Ok(chunk) = self.consumer.read_chunk(n) {
                chunk.commit_all();
            }
            return 0;
        }
        let len = out.len().min(self.fade_left);
        let n = self.consumer.pop_partial_slice(&mut out[..len]).0.len();
        for sample in &mut out[..n] {
            *sample *= self.fade_left as f32 / STOP_FADE as f32;
            self.fade_left -= 1;
        }
        n
    }

    /// Fills interleaved `out` with `channels` channels: each mono sample is copied to every channel, converted with
    /// `convert`, and silence follows when the feed runs dry.
    pub fn fill<T: Copy>(&mut self, out: &mut [T], channels: usize, convert: impl Fn(f32) -> T) {
        let channels = channels.max(1);
        let mut mono = [0.0f32; 256];
        let silence = convert(0.0);
        for frames in out.chunks_mut(channels * mono.len()) {
            let wanted = frames.len().div_ceil(channels);
            let got = self.pop(&mut mono[..wanted]);
            for (i, frame) in frames.chunks_mut(channels).enumerate() {
                frame.fill(if i < got { convert(mono[i]) } else { silence });
            }
        }
    }
}

/// Plays through the system's audio API with [`cpal`].
#[derive(Debug, Default)]
pub struct CpalOutput;

impl CpalOutput {
    pub fn new() -> Self {
        Self
    }
}

fn backend(e: impl fmt::Display) -> OutputError {
    OutputError::Backend(e.to_string())
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| device.to_string())
}

impl OutputBackend for CpalOutput {
    fn devices(&self) -> Result<Vec<OutputDevice>, OutputError> {
        let host = cpal::default_host();
        let default = host.default_output_device().and_then(|d| d.id().ok());
        let mut devices = Vec::new();
        for device in host.output_devices().map_err(backend)? {
            let Ok(id) = device.id() else { continue };
            devices.push(OutputDevice {
                is_default: Some(&id) == default.as_ref(),
                id: id.to_string(),
                name: device_name(&device),
            });
        }
        Ok(devices)
    }

    fn open(&self, id: Option<&str>, feed: OutputFeed) -> Result<Box<dyn OutputStream>, OutputError> {
        let host = cpal::default_host();
        let device = match id {
            Some(id) => {
                let not_found = || OutputError::NotFound(id.to_owned());
                let parsed = id.parse::<cpal::DeviceId>().map_err(|_| not_found())?;
                host.device_by_id(&parsed).ok_or_else(not_found)?
            }
            None => host.default_output_device().ok_or(OutputError::NoDevice)?,
        };
        let supported = device.default_output_config().map_err(backend)?;
        let config = supported.config();
        let channels = usize::from(config.channels);
        let failure = Arc::new(Mutex::new(None));
        let on_error = {
            let failure = Arc::clone(&failure);
            move |e: cpal::Error| {
                // Glitches and a default device that was followed to another one aren't the end of the stream.
                use cpal::ErrorKind::{DeviceChanged, RealtimeDenied, Xrun};
                if !matches!(e.kind(), Xrun | DeviceChanged | RealtimeDenied) {
                    *crate::lock(&failure) = Some(e.to_string());
                }
            }
        };
        let stream = build_stream(&device, supported.sample_format(), config, channels, feed, on_error)?;
        stream.play().map_err(backend)?;
        Ok(Box::new(CpalStream {
            _stream: stream,
            sample_rate: config.sample_rate,
            name: device_name(&device),
            failure,
        }))
    }
}

fn build_stream(
    device: &cpal::Device,
    format: cpal::SampleFormat,
    config: cpal::StreamConfig,
    channels: usize,
    feed: OutputFeed,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, OutputError> {
    use cpal::{FromSample, SampleFormat};

    fn build<T: cpal::SizedSample + FromSample<f32>>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        mut feed: OutputFeed,
        on_error: impl FnMut(cpal::Error) + Send + 'static,
    ) -> Result<cpal::Stream, OutputError> {
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    feed.fill(data, channels, <T as FromSample<f32>>::from_sample_)
                },
                on_error,
                None,
            )
            .map_err(backend)
    }

    match format {
        SampleFormat::F32 => build::<f32>(device, config, channels, feed, on_error),
        SampleFormat::I16 => build::<i16>(device, config, channels, feed, on_error),
        SampleFormat::I32 => build::<i32>(device, config, channels, feed, on_error),
        SampleFormat::U16 => build::<u16>(device, config, channels, feed, on_error),
        SampleFormat::F64 => build::<f64>(device, config, channels, feed, on_error),
        other => Err(OutputError::Backend(format!("unsupported sample format {other}"))),
    }
}

struct CpalStream {
    _stream: cpal::Stream,
    sample_rate: u32,
    name: String,
    failure: Arc<Mutex<Option<String>>>,
}

impl OutputStream for CpalStream {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn device_name(&self) -> &str {
        &self.name
    }

    fn failure(&self) -> Option<String> {
        crate::lock(&self.failure).clone()
    }
}

/// Pulls from a feed on its own thread, `speed` times faster than real time, and hands what it got to `sink`.
struct Puller {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Puller {
    fn start(
        sample_rate: u32,
        speed: f32,
        mut feed: OutputFeed,
        mut sink: impl FnMut(&[f32]) + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("rt-output".into())
            .spawn(move || {
                // 10 ms of audio per period, like a device callback.
                let mut block = vec![0.0; (sample_rate / 100).max(1) as usize];
                let period = Duration::from_secs_f64(0.01 / f64::from(speed.max(0.01)));
                let mut next = Instant::now();
                while !stopped.load(Ordering::Relaxed) {
                    let n = feed.pop(&mut block);
                    sink(&block[..n]);
                    next += period;
                    thread::sleep(next.saturating_duration_since(Instant::now()));
                }
                block.fill(0.0);
            })
            .expect("can't start an output thread");
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Puller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Plays into nothing, in real time: for running the app without an audio device (CI, the dev box).
#[derive(Debug, Clone)]
pub struct NullOutput {
    sample_rate: u32,
}

impl NullOutput {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }
}

impl Default for NullOutput {
    fn default() -> Self {
        Self::new(48_000)
    }
}

const NULL_DEVICE: &str = "null";

impl OutputBackend for NullOutput {
    fn devices(&self) -> Result<Vec<OutputDevice>, OutputError> {
        Ok(vec![OutputDevice {
            id: NULL_DEVICE.into(),
            name: "No output".into(),
            is_default: true,
        }])
    }

    fn open(&self, id: Option<&str>, feed: OutputFeed) -> Result<Box<dyn OutputStream>, OutputError> {
        if let Some(id) = id.filter(|&id| id != NULL_DEVICE) {
            return Err(OutputError::NotFound(id.into()));
        }
        Ok(Box::new(PulledStream {
            sample_rate: self.sample_rate,
            name: "No output".into(),
            puller: Some(Puller::start(self.sample_rate, 1.0, feed, |_| {})),
            on_drop: None,
            failure: None,
        }))
    }
}

struct PulledStream {
    sample_rate: u32,
    name: String,
    puller: Option<Puller>,
    /// Runs once the puller stopped.
    on_drop: Option<Box<dyn FnOnce()>>,
    failure: Option<Arc<AtomicBool>>,
}

impl OutputStream for PulledStream {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn device_name(&self) -> &str {
        &self.name
    }

    fn failure(&self) -> Option<String> {
        let failed = self.failure.as_ref().is_some_and(|f| f.load(Ordering::Relaxed));
        failed.then(|| "the test broke the stream".into())
    }
}

impl Drop for PulledStream {
    fn drop(&mut self) {
        drop(self.puller.take());
        if let Some(on_drop) = self.on_drop.take() {
            on_drop();
        }
    }
}

/// Collects everything played in memory, for tests. Clones share the collected audio.
///
/// Its devices are named after their ids; the first one is the default. It pulls `speed` times faster than real time,
/// and collects only what it pulls, never the silence of an empty feed.
#[derive(Clone)]
pub struct TestOutput {
    inner: Arc<TestInner>,
}

struct TestInner {
    sample_rate: u32,
    speed: f32,
    devices: Vec<String>,
    state: Mutex<TestState>,
}

#[derive(Default)]
struct TestState {
    samples: Vec<f32>,
    /// The device id asked for at every open, failed or not.
    opens: Vec<Option<String>>,
    open_streams: usize,
    failing: Vec<String>,
    fail_all: bool,
    /// Breaks the streams that are open now.
    broken: Vec<Arc<AtomicBool>>,
}

impl TestOutput {
    /// A real-time output at `sample_rate` with one device, "speakers".
    pub fn new(sample_rate: u32) -> Self {
        Self {
            inner: Arc::new(TestInner {
                sample_rate,
                speed: 1.0,
                devices: vec!["speakers".into()],
                state: Mutex::default(),
            }),
        }
    }

    /// Pulls `speed` times faster than real time.
    pub fn speed(self, speed: f32) -> Self {
        self.rebuild(|inner| inner.speed = speed)
    }

    /// Replaces the devices; the first one is the default.
    pub fn with_devices(self, ids: &[&str]) -> Self {
        self.rebuild(|inner| inner.devices = ids.iter().map(|&id| id.into()).collect())
    }

    fn rebuild(self, change: impl FnOnce(&mut TestInner)) -> Self {
        let mut inner = TestInner {
            sample_rate: self.inner.sample_rate,
            speed: self.inner.speed,
            devices: self.inner.devices.clone(),
            state: Mutex::default(),
        };
        change(&mut inner);
        Self { inner: Arc::new(inner) }
    }

    /// Opening the device `id` fails from now on.
    pub fn fail_device(&self, id: &str) {
        self.lock().failing.push(id.into());
    }

    /// Opening any device fails from now on.
    pub fn fail_all(&self) {
        self.lock().fail_all = true;
    }

    /// The streams open now fail, like an unplugged device.
    pub fn break_streams(&self) {
        for broken in self.lock().broken.drain(..) {
            broken.store(true, Ordering::Relaxed);
        }
    }

    /// Everything played so far.
    pub fn samples(&self) -> Vec<f32> {
        self.lock().samples.clone()
    }

    /// How many samples were played so far.
    pub fn len(&self) -> usize {
        self.lock().samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forgets what was played.
    pub fn clear(&self) {
        let mut state = self.lock();
        state.samples.fill(0.0);
        state.samples.clear();
    }

    /// The device id asked for at every open so far (`None`: the default).
    pub fn opens(&self) -> Vec<Option<String>> {
        self.lock().opens.clone()
    }

    /// Streams open right now.
    pub fn open_streams(&self) -> usize {
        self.lock().open_streams
    }

    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TestState> {
        crate::lock(&self.inner.state)
    }
}

impl OutputBackend for TestOutput {
    fn devices(&self) -> Result<Vec<OutputDevice>, OutputError> {
        Ok(self
            .inner
            .devices
            .iter()
            .enumerate()
            .map(|(i, id)| OutputDevice {
                id: id.clone(),
                name: id.clone(),
                is_default: i == 0,
            })
            .collect())
    }

    fn open(&self, id: Option<&str>, feed: OutputFeed) -> Result<Box<dyn OutputStream>, OutputError> {
        let mut state = self.lock();
        state.opens.push(id.map(String::from));
        let device = match id {
            Some(id) => self
                .inner
                .devices
                .iter()
                .find(|d| *d == id)
                .ok_or_else(|| OutputError::NotFound(id.into()))?,
            None => self.inner.devices.first().ok_or(OutputError::NoDevice)?,
        };
        if state.fail_all || state.failing.contains(device) {
            return Err(OutputError::Backend(format!("{device} failed to open")));
        }
        state.open_streams += 1;
        let broken = Arc::new(AtomicBool::new(false));
        state.broken.push(Arc::clone(&broken));
        drop(state);

        let collect = self.clone();
        let close = self.clone();
        Ok(Box::new(PulledStream {
            sample_rate: self.inner.sample_rate,
            name: device.clone(),
            puller: Some(Puller::start(
                self.inner.sample_rate,
                self.inner.speed,
                feed,
                move |samples| {
                    if !samples.is_empty() {
                        collect.lock().samples.extend_from_slice(samples);
                    }
                },
            )),
            on_drop: Some(Box::new(move || close.lock().open_streams -= 1)),
            failure: Some(broken),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_mono_to_every_channel_and_pads_with_silence() {
        let (mut writer, mut feed) = OutputFeed::new(16);
        assert_eq!(writer.write(&[0.1, 0.2, 0.3]), 3);
        assert_eq!(writer.queued(), 3);
        let mut out = [9i32; 8];
        feed.fill(&mut out, 2, |s| (s * 10.0).round() as i32);
        assert_eq!(out, [1, 1, 2, 2, 3, 3, 0, 0]);
        assert_eq!(writer.queued(), 0);
    }

    #[test]
    fn fills_large_buffers() {
        let (mut writer, mut feed) = OutputFeed::new(4096);
        let input: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        writer.write(&input);
        let mut out = vec![0.0f32; 3000];
        feed.fill(&mut out, 3, |s| s);
        assert!(out.chunks(3).enumerate().all(|(i, f)| f == [i as f32; 3]));
    }

    #[test]
    fn writes_only_what_fits() {
        let (mut writer, _feed) = OutputFeed::new(4);
        assert_eq!(writer.write(&[1.0; 6]), 4);
        assert_eq!(writer.write(&[1.0]), 0);
    }

    #[test]
    fn a_stop_fades_out_and_drops_the_rest() {
        let (mut writer, mut feed) = OutputFeed::new(4096);
        writer.write(&[1.0; 4096]);
        let mut out = [0.0; 100];
        assert_eq!(feed.pop(&mut out), 100);
        writer.stop();
        let mut faded = Vec::new();
        loop {
            let n = feed.pop(&mut out);
            if n == 0 {
                break;
            }
            faded.extend_from_slice(&out[..n]);
        }
        assert_eq!(faded.len(), STOP_FADE);
        assert!(faded.windows(2).all(|w| w[1] < w[0]), "fades down");
        assert!(faded[0] > 0.99 && faded[STOP_FADE - 1] < 0.01);
        assert_eq!(writer.queued(), 0);
    }

    #[test]
    fn test_output_collects_what_it_pulls() {
        let output = TestOutput::new(1000).speed(10.0);
        let (mut writer, feed) = OutputFeed::new(1000);
        writer.write(&[0.5; 300]);
        let stream = output.open(None, feed).unwrap();
        assert_eq!(stream.device_name(), "speakers");
        assert_eq!(output.open_streams(), 1);
        let start = Instant::now();
        while output.len() < 300 && start.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(5));
        }
        // 300 ms of audio at 10x: about 30 ms.
        assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
        thread::sleep(Duration::from_millis(20));
        assert_eq!(output.samples(), vec![0.5; 300]);
        drop(stream);
        assert_eq!(output.open_streams(), 0);
    }

    #[test]
    fn test_output_devices_fail_on_request() {
        let output = TestOutput::new(1000).with_devices(&["speakers", "headphones"]);
        let devices = output.devices().unwrap();
        assert_eq!(devices.len(), 2);
        assert!(devices[0].is_default && !devices[1].is_default);

        let open = |id| {
            output
                .open(id, OutputFeed::new(10).1)
                .map(|s| s.device_name().to_owned())
        };
        assert_eq!(open(Some("headphones")).unwrap(), "headphones");
        assert_eq!(open(Some("usb")).unwrap_err(), OutputError::NotFound("usb".into()));
        output.fail_device("headphones");
        assert!(matches!(open(Some("headphones")), Err(OutputError::Backend(_))));
        assert_eq!(open(None).unwrap(), "speakers");
        output.fail_all();
        assert!(open(None).is_err());
        assert_eq!(output.opens().len(), 5);
    }

    #[test]
    fn broken_streams_report_a_failure() {
        let output = TestOutput::new(1000);
        let stream = output.open(None, OutputFeed::new(10).1).unwrap();
        assert_eq!(stream.failure(), None);
        output.break_streams();
        assert!(stream.failure().is_some());
    }

    #[test]
    fn null_output_has_one_device() {
        let output = NullOutput::default();
        assert_eq!(output.devices().unwrap().len(), 1);
        assert!(output.open(Some("speakers"), OutputFeed::new(10).1).is_err());
        let stream = output.open(None, OutputFeed::new(10).1).unwrap();
        assert_eq!(stream.sample_rate(), 48_000);
    }
}
