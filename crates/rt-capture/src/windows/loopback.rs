//! WASAPI process loopback: the audio one process tree plays, before it is mixed with other apps.

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient,
    IAudioClient, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{HRESULT, IUnknown, Interface, Ref};
use windows_core::implement;

use super::{Com, backend};
use crate::{CaptureError, CaptureFormat, CaptureHandle, FrameSink, Result, ThreadCapture};

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

/// Starts capturing `pid` and all its child processes on a new thread. Returns once the stream runs, or with the
/// reason it couldn't start.
pub(super) fn start(pid: u32, sink: FrameSink) -> Result<Box<dyn CaptureHandle>> {
    ensure_process_exists(pid)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("wasapi-capture".into())
        .spawn(move || run(pid, sink, &stopped, ready_tx))
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

/// The capture thread. Reports setup through `ready`, then delivers audio until `stop` is set or the stream fails.
fn run(pid: u32, mut sink: FrameSink, stop: &AtomicBool, ready: Sender<Result<()>>) {
    let stream = match Stream::open(pid) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut frames = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        if unsafe { WaitForSingleObject(stream.event, STOP_POLL_MS) } != WAIT_OBJECT_0 {
            continue;
        }
        // The stream can fail later on, e.g. when the audio service restarts. The handle then reports that capture is
        // no longer running.
        if stream.drain(&mut frames, &mut sink).is_err() {
            break;
        }
    }
}

/// A started process loopback stream, owned by the capture thread.
struct Stream {
    // Field order is drop order: the COM objects go before COM itself.
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
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
            let event = CreateEventW(None, false, false, None).map_err(backend("CreateEventW"))?;
            let stream = Self {
                capture: client.GetService().map_err(backend("IAudioCaptureClient"))?,
                client,
                event,
                _com: com,
            };
            stream
                .client
                .SetEventHandle(stream.event)
                .map_err(backend("SetEventHandle"))?;
            stream.client.Start().map_err(backend("IAudioClient::Start"))?;
            Ok(stream)
        }
    }

    /// Hands every waiting packet to `sink`.
    fn drain(&self, frames: &mut Vec<f32>, sink: &mut FrameSink) -> windows::core::Result<()> {
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut count = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut count, &mut flags, None, None)?;
                let len = count as usize * usize::from(FORMAT.channels);
                frames.clear();
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    frames.resize(len, 0.0);
                } else {
                    // The buffer is only guaranteed to be byte aligned.
                    let bytes = std::slice::from_raw_parts(data, len * 4);
                    frames.extend(bytes.as_chunks::<4>().0.iter().map(|&b| f32::from_ne_bytes(b)));
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

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
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
