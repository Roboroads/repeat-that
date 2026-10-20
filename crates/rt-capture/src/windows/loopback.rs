//! WASAPI process loopback: the audio one process tree plays, before it is mixed with other apps.

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForMultipleObjects,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{HRESULT, IUnknown, Interface, Owned, Ref};
use windows_core::implement;

use super::{Com, backend};
use crate::{CaptureError, CaptureFormat, CaptureHandle, DuckChange, DuckLevel, FrameSink, Result, ThreadCapture};

/// Process loopback has no mix format of its own; the audio engine converts to whatever we ask for.
const FORMAT: CaptureFormat = CaptureFormat {
    sample_rate: 48_000,
    channels: 2,
};
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// Shared-mode buffer: 200 ms, in 100 ns units. Packets arrive every ~10 ms; the rest is slack for a busy thread.
const BUFFER_DURATION: i64 = 2_000_000;
/// How often the capture thread checks whether it should stop when no audio arrives.
const STOP_POLL_MS: u32 = 100;
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a capture ducks sessions its app started while ducked.
pub(super) const REDUCK_INTERVAL: Duration = Duration::from_millis(500);

/// Starts capturing `pid` and all its child processes on a new thread, amplified by the inverse of `level` to undo
/// ducking. Calls `reduck` every [`REDUCK_INTERVAL`] on the capture thread. Returns once the stream runs, or with the
/// reason it couldn't start.
pub(super) fn start(
    pid: u32,
    level: DuckLevel,
    sink: FrameSink,
    reduck: Box<dyn FnMut() + Send>,
) -> Result<Box<dyn CaptureHandle>> {
    ensure_process_exists(pid)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("wasapi-capture".into())
        .spawn(move || run(pid, &level, sink, reduck, &stopped, ready_tx))
        .map_err(|e| CaptureError::Backend(format!("can't start capture thread: {e}")))?;
    let handle = ThreadCapture::new(stop, thread);
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Box::new(handle)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(CaptureError::Backend("capture thread ended during setup".into())),
    }
}

fn ensure_process_exists(pid: u32) -> Result<()> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|_| CaptureError::AppNotFound(pid))?;
    let _ = unsafe { CloseHandle(process) };
    Ok(())
}

/// The capture thread. Reports setup through `ready`, then delivers audio until `stop` is set, the stream fails or the
/// process exits.
fn run(
    pid: u32,
    level: &DuckLevel,
    mut sink: FrameSink,
    mut reduck: Box<dyn FnMut() + Send>,
    stop: &AtomicBool,
    ready: Sender<Result<()>>,
) {
    // The stream outlives the process and just goes silent, so the thread watches the process itself.
    let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(process) => unsafe { Owned::new(process) },
        Err(_) => {
            let _ = ready.send(Err(CaptureError::AppNotFound(pid)));
            return;
        }
    };
    let stream = match Stream::open(pid) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let waits = [*stream.event, *process];
    let (mut frames, mut history) = (Vec::new(), Vec::new());
    let mut reducked = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if reducked.elapsed() >= REDUCK_INTERVAL {
            reduck();
            reducked = Instant::now();
        }
        match unsafe { WaitForMultipleObjects(&waits, false, STOP_POLL_MS) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => continue,
            // The process exited (or waiting failed). The handle then reports that capture is no longer running.
            _ => break,
        }
        // The stream can fail later on, e.g. when the audio service restarts. The handle then reports that capture is
        // no longer running.
        if stream.drain(level, &mut history, &mut frames, &mut sink).is_err() {
            break;
        }
    }
}

/// A started process loopback stream, owned by the capture thread.
struct Stream {
    // Field order is drop order: the COM objects go before COM itself.
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Owned<HANDLE>,
    _com: Com,
}

impl Stream {
    fn open(pid: u32) -> Result<Self> {
        let com = Com::init()?;
        let client = activate(pid)?;
        let block_align = FORMAT.channels * 4;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: FORMAT.channels,
            nSamplesPerSec: FORMAT.sample_rate,
            nAvgBytesPerSec: FORMAT.sample_rate * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        unsafe {
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    BUFFER_DURATION,
                    0,
                    &format,
                    None,
                )
                .map_err(backend("IAudioClient::Initialize"))?;
            // Owned, so it is closed whichever step below fails.
            let event = Owned::new(CreateEventW(None, false, false, None).map_err(backend("CreateEventW"))?);
            let stream = Self {
                capture: client.GetService().map_err(backend("IAudioCaptureClient"))?,
                client,
                event,
                _com: com,
            };
            stream
                .client
                .SetEventHandle(*stream.event)
                .map_err(backend("SetEventHandle"))?;
            stream.client.Start().map_err(backend("IAudioClient::Start"))?;
            Ok(stream)
        }
    }

    /// Hands every waiting packet to `sink`, amplified by `1 / level`. The engine timestamps each packet, so the gain
    /// switches at the sample where the app's volume changed.
    fn drain(
        &self,
        level: &DuckLevel,
        history: &mut Vec<DuckChange>,
        frames: &mut Vec<f32>,
        sink: &mut FrameSink,
    ) -> windows::core::Result<()> {
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut count = 0u32;
                let mut flags = 0u32;
                let mut time = 0u64;
                self.capture
                    .GetBuffer(&mut data, &mut count, &mut flags, None, Some(&mut time))?;
                let len = count as usize * usize::from(FORMAT.channels);
                frames.clear();
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    frames.resize(len, 0.0);
                } else {
                    // The buffer is only guaranteed to be byte aligned.
                    let bytes = std::slice::from_raw_parts(data, len * 4);
                    frames.extend(bytes.as_chunks::<4>().0.iter().map(|&b| f32::from_ne_bytes(b)));
                    let timed = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0;
                    level.history(history);
                    compensate(frames, history, timed.then_some(time));
                }
                self.capture.ReleaseBuffer(count)?;
                if !frames.is_empty() {
                    sink(frames, FORMAT);
                }
            }
        }
        Ok(())
    }
}

/// Undoes ducking on a packet that starts at `time` (100 ns units): each frame is amplified by the inverse of the
/// level from `history` (oldest first) that applied when it was played. Without a timestamp the whole packet gets the
/// latest level.
fn compensate(frames: &mut [f32], history: &[DuckChange], time: Option<u64>) {
    let Some(time) = time else {
        let level = history.last().map_or(1.0, |c| c.level);
        amplify(frames, level);
        return;
    };
    let channels = usize::from(FORMAT.channels);
    // The level at the start of the packet, then every change during it.
    let first = history.iter().rposition(|c| c.since <= time).unwrap_or(0);
    let mut level = history.get(first).map_or(1.0, |c| c.level);
    let mut start = 0;
    for change in history.iter().skip(first + 1) {
        let ticks = u128::from(change.since - time) * u128::from(FORMAT.sample_rate);
        let frame = usize::try_from(ticks.div_ceil(10_000_000)).unwrap_or(usize::MAX);
        let end = frame.saturating_mul(channels).min(frames.len());
        amplify(&mut frames[start..end], level);
        (start, level) = (end, change.level);
    }
    amplify(&mut frames[start..], level);
}

fn amplify(frames: &mut [f32], level: f32) {
    if level < 1.0 {
        let gain = 1.0 / level;
        frames.iter_mut().for_each(|s| *s *= gain);
    }
}

/// How long after `SetMasterVolume` returns the audio engine plays the app at the new volume. Windows doesn't report
/// this. With a 10 ms engine period it measured 30–37 ms for single changes, and the steps of a fade landed anywhere
/// from 10 ms before this estimate to 7 ms after it, by the same amount for every step of one fade: presumably where
/// the fade's calls fall in the engine's period. Ducking fades in half-dB steps 10 ms apart, so that is inaudible.
const VOLUME_LATENCY: u64 = 330_000;

/// When a volume set right now takes effect, on the clock of the packet timestamps.
pub(super) fn volume_takes_effect() -> u64 {
    now() + VOLUME_LATENCY
}

/// The current time on the clock of the packet timestamps: the performance counter in 100 ns units.
fn now() -> u64 {
    let (mut counter, mut frequency) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    if frequency <= 0 {
        return 0;
    }
    (i128::from(counter) * 10_000_000 / i128::from(frequency)) as u64
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// Activates an `IAudioClient` for process loopback of `pid`'s process tree. Activation is asynchronous; this waits
/// for it.
fn activate(pid: u32) -> Result<IAudioClient> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // The PROPVARIANT only borrows `params`, so it must not be cleared (which would free the blob).
    let prop = ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&raw mut params).cast(),
                    },
                },
            }),
        },
    });

    let (done_tx, done_rx) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Activated(Mutex::new(Some(done_tx))).into();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const *prop),
            &handler,
        )
    }
    .map_err(activation_error)?;
    done_rx
        .recv_timeout(ACTIVATION_TIMEOUT)
        .map_err(|_| CaptureError::Backend("process loopback activation timed out".into()))?;

    let mut result = HRESULT(0);
    let mut client: Option<IUnknown> = None;
    unsafe { operation.GetActivateResult(&mut result, &mut client) }.map_err(backend("GetActivateResult"))?;
    result.ok().map_err(activation_error)?;
    client
        .ok_or_else(|| CaptureError::Backend("process loopback activation returned nothing".into()))?
        .cast()
        .map_err(backend("IAudioClient"))
}

fn activation_error(e: windows::core::Error) -> CaptureError {
    CaptureError::Backend(format!(
        "process loopback activation failed (needs Windows 10 version 2004 or later): {e}"
    ))
}

/// Signals the waiting thread when activation completes. COM calls this on one of its own threads.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(Mutex<Option<Sender<()>>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _operation: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        if let Some(done) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = done.send(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = 10_000_000;

    /// Full volume, ducked to 25% at 1 s.
    const DUCKED_AT_1S: [DuckChange; 2] = [
        DuckChange { since: 0, level: 1.0 },
        DuckChange {
            since: SECOND,
            level: 0.25,
        },
    ];

    /// The time of `frames` frames at 48 kHz, rounded down.
    fn frames(frames: u64) -> u64 {
        frames * SECOND / 48_000
    }

    #[test]
    fn switches_gain_at_the_sample_where_the_volume_changed() {
        // 10 stereo frames starting 5 frames before the change.
        let mut samples = vec![0.25f32; 20];
        compensate(&mut samples, &DUCKED_AT_1S, Some(SECOND - frames(5)));
        // The frame clock rounds, so the switch lands on frame 5 or 6.
        let first_boosted = samples.iter().position(|&s| s == 1.0).unwrap();
        assert!(first_boosted == 10 || first_boosted == 12, "{samples:?}");
        assert!(samples[..first_boosted].iter().all(|&s| s == 0.25));
        assert!(samples[first_boosted..].iter().all(|&s| s == 1.0));
    }

    #[test]
    fn follows_every_step_of_a_ramp() {
        let history = [
            DuckChange { since: 0, level: 1.0 },
            DuckChange {
                since: SECOND + frames(2),
                level: 0.5,
            },
            DuckChange {
                since: SECOND + frames(4),
                level: 0.25,
            },
        ];
        // Mono-equivalent check on the first channel: 1.0 → 2× → 4×.
        let mut samples = vec![0.25f32; 12];
        compensate(&mut samples, &history, Some(SECOND));
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        assert_eq!(left[0], 0.25);
        assert_eq!(left[5], 1.0);
        assert!(left.windows(2).all(|w| w[0] <= w[1]), "{left:?}");
        assert!(left.contains(&0.5), "{left:?}");
    }

    #[test]
    fn packets_after_the_change_get_the_new_gain() {
        let mut samples = vec![0.25f32; 8];
        compensate(&mut samples, &DUCKED_AT_1S, Some(2 * SECOND));
        assert!(samples.iter().all(|&s| s == 1.0));
    }

    #[test]
    fn packets_before_the_change_keep_the_old_gain() {
        let mut samples = vec![0.5f32; 8];
        compensate(&mut samples, &DUCKED_AT_1S, Some(1_000));
        assert!(samples.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn late_packets_from_before_a_duck_and_restore_keep_their_gain() {
        // Ducked at 1 s and restored at 1.1 s; a packet from 0.5 s drained afterwards was played at full volume.
        let history = [
            DUCKED_AT_1S[0],
            DUCKED_AT_1S[1],
            DuckChange {
                since: SECOND + SECOND / 10,
                level: 1.0,
            },
        ];
        let mut samples = vec![0.5f32; 8];
        compensate(&mut samples, &history, Some(SECOND / 2));
        assert!(samples.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn without_a_timestamp_the_current_level_applies() {
        let mut samples = vec![0.25f32; 8];
        compensate(&mut samples, &DUCKED_AT_1S, None);
        assert!(samples.iter().all(|&s| s == 1.0));
    }
}
