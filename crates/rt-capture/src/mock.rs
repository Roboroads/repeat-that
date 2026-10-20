//! A fake app that plays audio from memory or a WAV file, for developing and testing on any OS.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    AppAudioSource, AudioApp, CaptureError, CaptureFormat, CaptureHandle, DuckLevel, FrameSink, Result, ThreadCapture,
};

/// Frames are delivered in 10 ms chunks, like a real capture callback.
const CHUNK: Duration = Duration::from_millis(10);

/// Pretends to be a single app that plays the given audio.
///
/// By default the audio is delivered in real time and loops forever. Tests can turn both off to get every sample once,
/// as fast as possible.
pub struct MockSource {
    app: AudioApp,
    samples: Arc<[f32]>,
    format: CaptureFormat,
    realtime: bool,
    looping: bool,
    level: DuckLevel,
}

impl MockSource {
    /// The PID of the fake app.
    pub const PID: u32 = u32::MAX;

    /// A fake app named `name` that plays interleaved `samples` in `format`.
    pub fn new(name: impl Into<String>, samples: Vec<f32>, format: CaptureFormat) -> Self {
        Self {
            app: AudioApp {
                pid: Self::PID,
                name: name.into(),
                exe_path: None,
            },
            samples: samples.into(),
            format,
            realtime: true,
            looping: true,
            level: DuckLevel::new(),
        }
    }

    /// A fake app that plays a WAV file, named after the file. The file is only read.
    pub fn from_wav(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let wav_error = |e: hound::Error| CaptureError::Backend(format!("{}: {e}", path.display()));
        let mut reader = hound::WavReader::open(path).map_err(wav_error)?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<std::result::Result<_, _>>(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|s| s as f32 * scale))
                    .collect::<std::result::Result<_, _>>()
            }
        }
        .map_err(wav_error)?;
        let name = path
            .file_stem()
            .map_or_else(|| "WAV".into(), |s| s.to_string_lossy().into_owned());
        let format = CaptureFormat {
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        };
        Ok(Self::new(name, samples, format))
    }

    /// Whether to pace delivery like a real app (default) or deliver as fast as the sink takes it.
    pub fn realtime(mut self, realtime: bool) -> Self {
        self.realtime = realtime;
        self
    }

    /// Whether to start over at the end (default) or stop delivering.
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    /// The level last set with [`AppAudioSource::set_app_volume`]. It doesn't change the delivered audio.
    pub fn level(&self) -> f32 {
        self.level.get()
    }

    fn check(&self, app: &AudioApp) -> Result<()> {
        if app.pid == self.app.pid {
            Ok(())
        } else {
            Err(CaptureError::AppNotFound(app.pid))
        }
    }
}

impl AppAudioSource for MockSource {
    fn list_apps(&self) -> Result<Vec<AudioApp>> {
        Ok(vec![self.app.clone()])
    }

    fn start(&self, app: &AudioApp, mut sink: FrameSink) -> Result<Box<dyn CaptureHandle>> {
        self.check(app)?;
        let samples = Arc::clone(&self.samples);
        let format = self.format;
        let (realtime, looping) = (self.realtime, self.looping);
        let chunk_len = (format.sample_rate as usize / 100 * format.channels as usize).max(format.channels as usize);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);

        let thread = thread::Builder::new()
            .name("mock-capture".into())
            .spawn(move || {
                let mut next = Instant::now();
                let mut pos = 0;
                while !stopped.load(Ordering::Relaxed) && !samples.is_empty() {
                    if pos >= samples.len() {
                        if !looping {
                            break;
                        }
                        pos = 0;
                    }
                    let end = (pos + chunk_len).min(samples.len());
                    sink(&samples[pos..end], format);
                    pos = end;
                    if realtime {
                        next += CHUNK;
                        thread::sleep(next.saturating_duration_since(Instant::now()));
                    }
                }
            })
            .map_err(|e| CaptureError::Backend(format!("can't start capture thread: {e}")))?;
        Ok(Box::new(ThreadCapture::new(stop, thread)))
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()> {
        self.check(app)?;
        self.level.set(level, 0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const STEREO_48K: CaptureFormat = CaptureFormat {
        sample_rate: 48_000,
        channels: 2,
    };

    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32 / len as f32).collect()
    }

    fn collect(source: &MockSource) -> Vec<f32> {
        let app = &source.list_apps().unwrap()[0];
        let (tx, rx) = mpsc::channel();
        let handle = source
            .start(
                app,
                Box::new(move |frames, format| {
                    assert_eq!(format, STEREO_48K);
                    tx.send(frames.to_vec()).unwrap();
                }),
            )
            .unwrap();
        // The sender is dropped when the thread finishes, which ends the iterator.
        let out = rx.iter().flatten().collect();
        handle.stop();
        out
    }

    #[test]
    fn lists_one_app() {
        let source = MockSource::new("Discord", vec![], STEREO_48K);
        let apps = source.list_apps().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Discord");
        assert_eq!(apps[0].pid, MockSource::PID);
    }

    #[test]
    fn delivers_every_sample_once_in_10ms_chunks() {
        let samples = ramp(48_000 * 2 / 4 + 7);
        let source = MockSource::new("app", samples.clone(), STEREO_48K)
            .realtime(false)
            .looping(false);
        assert_eq!(collect(&source), samples);
    }

    #[test]
    fn loops_until_stopped() {
        let samples = ramp(960);
        let source = MockSource::new("app", samples.clone(), STEREO_48K).realtime(false);
        let app = &source.list_apps().unwrap()[0];
        let (tx, rx) = mpsc::sync_channel(0);
        let handle = source
            .start(app, Box::new(move |frames, _| _ = tx.try_send(frames.to_vec())))
            .unwrap();
        // Delivery keeps going well past the end of the samples.
        let received: Vec<f32> = rx.iter().take(5).flatten().collect();
        handle.stop();
        assert_eq!(received.len(), 5 * samples.len());
        assert_eq!(&received[..960], &samples[..]);
        assert_eq!(&received[960..1920], &samples[..]);
    }

    #[test]
    fn realtime_delivery_is_paced() {
        let source = MockSource::new("app", ramp(48_000 * 2 / 10), STEREO_48K).looping(false);
        let start = Instant::now();
        collect(&source);
        // 100 ms of audio; allow for a slow CI machine on the upper side only.
        assert!(start.elapsed() >= Duration::from_millis(90), "{:?}", start.elapsed());
    }

    #[test]
    fn records_duck_level() {
        let source = MockSource::new("app", vec![], STEREO_48K);
        let app = source.list_apps().unwrap().remove(0);
        source.set_app_volume(&app, 0.25).unwrap();
        assert_eq!(source.level(), 0.25);
        source.set_app_volume(&app, 0.0).unwrap();
        assert_eq!(source.level(), crate::MIN_DUCK_LEVEL);
        source.set_app_volume(&app, f32::NAN).unwrap();
        assert_eq!(source.level(), 1.0);
        source.set_app_volume(&app, 2.0).unwrap();
        assert_eq!(source.level(), 1.0);
    }

    #[test]
    fn rejects_other_apps() {
        let source = MockSource::new("app", vec![0.0; 4], STEREO_48K);
        let other = AudioApp {
            pid: 42,
            name: "other".into(),
            exe_path: None,
        };
        assert!(matches!(
            source.start(&other, Box::new(|_, _| {})),
            Err(CaptureError::AppNotFound(42))
        ));
        assert!(matches!(
            source.set_app_volume(&other, 0.5),
            Err(CaptureError::AppNotFound(42))
        ));
    }

    #[test]
    fn reads_wav_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rt-core/tests/fixtures/speech_8s.wav");
        let source = MockSource::from_wav(&path).unwrap().realtime(false).looping(false);
        assert_eq!(source.list_apps().unwrap()[0].name, "speech_8s");
        assert_eq!(source.format.sample_rate, 16_000);
        assert_eq!(source.format.channels, 1);
        assert_eq!(source.samples.len(), 8 * 16_000);
        assert!(source.samples.iter().any(|s| s.abs() > 0.1));
    }
}
