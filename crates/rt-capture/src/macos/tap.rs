//! One app's process tap: a private tap of the app's processes, in a private aggregate device whose main subdevice is
//! the app's own output device. Its IOProc hands the tapped audio to a capture, and plays the replay while the app is
//! ducked (see `passthrough`). While the app is ducked, a second tap in an aggregate of its own mutes it (see
//! `Description`). A watcher thread keeps the taps' process lists up to date, and ends the tap when the app quits, its
//! device goes away, or the user takes back the permission.

use std::ffi::{CStr, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioHardwareIOProcStreamUsage, AudioObjectAddPropertyListener, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectRemovePropertyListener, CATapDescription, CATapMuteBehavior,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey, kAudioAggregateDeviceMainSubDeviceKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyIOProcStreamUsage,
    kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyScopeInput, kAudioSubDeviceUIDKey,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyDescription, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, AudioTimeStampFlags,
    kAudioFormatFlagIsFloat, kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFString, CFType};
use objc2_foundation::{NSArray, NSNumber, NSString, NSUUID};

use super::passthrough::{self, Buffer, BufferMut, Command, IoStatus, Player, Sequencer, Step};
use super::processes::{self, Follow};
use super::{permission, sys};
use crate::permission::PermissionStatus;
use crate::{CaptureError, CaptureFormat, CaptureHandle, FrameSink, Result};

/// How often the watcher looks at the app's processes when Core Audio doesn't say they changed.
const WATCH_EVERY: Duration = Duration::from_millis(500);
/// How often the watcher checks that the user still allows capture.
const PERMISSION_EVERY: Duration = Duration::from_secs(2);
/// A tap whose IOProc hasn't run for this long has stopped: its device is gone or asleep.
const IO_STALLED: Duration = Duration::from_secs(3);
/// How long a new tap's device may take to start.
const START_TIMEOUT: Duration = Duration::from_secs(2);
/// A duck whose IOProc hasn't moved for this long can't be heard: the app is muted or unmuted without a replay.
const DUCK_STALLED: Duration = Duration::from_millis(500);
/// The longest a duck may take before it gives up on the replay the same way. A full fade takes about 200 ms.
const DUCK_TIMEOUT: Duration = Duration::from_secs(2);
/// How often a duck looks at the IOProc's progress.
const DUCK_POLL: Duration = Duration::from_millis(2);
/// The most frames one IO cycle is expected to have; longer cycles allocate once.
const MAX_CYCLE: usize = 8192;
/// The most buffers an IO cycle's buffer list is read from, per direction.
const MAX_BUFFERS: usize = 32;

/// A running tap. Dropping it stops the IOProc and removes the aggregate device and the tap, and with it any mute.
pub(crate) struct Tap {
    /// Only dropped: it removes the IOProc, the aggregate and the tap.
    _parts: Parts,
    shared: Arc<Shared>,
    watcher: Option<JoinHandle<()>>,
    /// The ducking state machine, held for a whole duck so ducks don't interleave.
    sequencer: Mutex<Sequencer>,
}

/// What the IOProc, the watcher and the control side share.
struct Shared {
    /// The app's PID, as it was listed.
    pid: u32,
    /// How the watcher finds the app's processes as they come and go.
    follow: Follow,
    tap: AudioObjectID,
    /// The app's output device, the aggregate's main subdevice.
    device: AudioObjectID,
    format: CaptureFormat,
    /// The latest [`Command`] for the replay, packed.
    command: AtomicU64,
    /// Frames the IOProc has played, and the replay's gain after them, for [`IoStatus`].
    frames: AtomicU64,
    gain: AtomicU32,
    /// How far ahead of the tap's input the IOProc writes its output, in frames, from the latest cycle's timestamps.
    /// The replay is heard about that much later than the app's own audio: both go out through the same device, so its
    /// output latency adds to both. On CI (BlackHole) the replay was heard 45 ms late with 64 ms ahead.
    ahead: AtomicU64,
    /// Only the IOProc locks this, except while the tap is set up.
    io: Mutex<Io>,
    /// The capture's sink and the number it was attached with, see [`Capture`].
    sink: Mutex<Option<(u64, FrameSink)>>,
    sinks: AtomicU64,
    /// The tap ended by itself: the app quit, its device went away, or capture isn't allowed any more.
    ended: AtomicBool,
    stop: AtomicBool,
    /// Wakes the watcher when processes come or go, or when it should stop.
    wake: Wake,
    description: Mutex<Description>,
}

struct Io {
    player: Player,
    /// The tapped audio of the current cycle, interleaved.
    tapped: Vec<f32>,
}

/// What the taps are made from, so they can be changed: the app's processes, and the tap that mutes them while the app
/// is ducked.
///
/// The mute is a tap of its own, made when the app is muted and removed when it is unmuted. Muting through the
/// capture's own tap means changing that tap's description, and CI then heard a 35 ms gap in the replay right when the
/// app was muted: the tap's audio drops out while it changes. A tap only mutes while it is read (CI found that too), so
/// the muting tap gets an aggregate device of its own, which runs while it exists. The capture's tap still changes when
/// the app's processes do (see `watch`), which may cost the capture such a dropout then.
struct Description {
    uuid: String,
    processes: Vec<AudioObjectID>,
    /// The muting tap with its aggregate device, and its UUID, while the app is muted.
    mute: Option<(Parts, String)>,
}

impl Description {
    /// A private stereo tap of the app's processes, with `uuid`.
    fn build(&self, uuid: &str, muted: bool) -> Retained<CATapDescription> {
        let numbers: Vec<Retained<NSNumber>> = self.processes.iter().map(|&id| NSNumber::new_u32(id)).collect();
        let processes = NSArray::from_retained_slice(&numbers);
        // SAFETY: an array of numbers holding process object IDs, as the initializer wants; the setters take plain
        // values.
        unsafe {
            let description = CATapDescription::initStereoMixdownOfProcesses(CATapDescription::alloc(), &processes);
            if let Some(uuid) = NSUUID::initWithUUIDString(NSUUID::alloc(), &NSString::from_str(uuid)) {
                description.setUUID(&uuid);
            }
            description.setName(&NSString::from_str("Repeat That"));
            description.setPrivate(true);
            description.setMuteBehavior(if muted {
                CATapMuteBehavior::Muted
            } else {
                CATapMuteBehavior::Unmuted
            });
            description
        }
    }

    /// Makes a tap of the app's processes.
    fn create(&self, uuid: &str, muted: bool) -> Result<AudioObjectID> {
        let mut tap = 0;
        let status = autoreleasepool(|_| {
            let built = self.build(uuid, muted);
            // SAFETY: a valid description and a place for the ID.
            unsafe { AudioHardwareCreateProcessTap(Some(&built), &mut tap) }
        });
        sys::check("AudioHardwareCreateProcessTap", status)?;
        Ok(tap)
    }

    /// Gives the capture's tap `tap`, and the muting tap if there is one, the current processes.
    fn apply(&self, tap: AudioObjectID) -> Result<()> {
        let change = |tap: AudioObjectID, uuid: &str, muted: bool| {
            autoreleasepool(|_| {
                let description = self.build(uuid, muted);
                let pointer: *const CATapDescription = &*description;
                // SAFETY: the property is a CATapDescription reference, which Core Audio retains as it needs.
                unsafe { sys::set(tap, sys::global(kAudioTapPropertyDescription), &pointer) }
            })
        };
        let captured = change(tap, &self.uuid, false);
        if let Some((Parts { tap: Some(mute), .. }, uuid)) = &self.mute {
            change(*mute, uuid, true)?;
        }
        captured
    }

    /// Mutes (`true`) or unmutes the app by making or removing the muting tap, reading it through an aggregate device
    /// on `device`.
    fn set_muted(&mut self, muted: bool, device: AudioObjectID) -> Result<()> {
        if !muted {
            // Dropping the parts stops the aggregate and removes it and the tap.
            self.mute = None;
            return Ok(());
        }
        if self.mute.is_some() {
            return Ok(());
        }
        let uuid = new_uuid();
        let mut parts = Parts::default();
        parts.tap = Some(self.create(&uuid, true)?);
        let aggregate = create_aggregate(&uuid, &*sys::device_uid(device)?)?;
        parts.aggregate = Some(aggregate);
        // SAFETY: an IOProc that needs no client data.
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                aggregate,
                Some(silence),
                std::ptr::null_mut(),
                NonNull::from(&mut parts.io_proc),
            )
        };
        sys::check("AudioDeviceCreateIOProcID", status)?;
        use_only_the_tap(aggregate, parts.io_proc);
        // SAFETY: the IOProc was just made on this device.
        sys::check("AudioDeviceStart", unsafe {
            AudioDeviceStart(aggregate, parts.io_proc)
        })?;
        parts.started = true;
        self.mute = Some((parts, uuid));
        Ok(())
    }
}

fn new_uuid() -> String {
    autoreleasepool(|_| NSUUID::new().UUIDString().to_string())
}

/// Sleeps until woken or a timeout, for the watcher.
#[derive(Default)]
struct Wake {
    woken: Mutex<bool>,
    condvar: Condvar,
}

impl Wake {
    fn wake(&self) {
        *lock(&self.woken) = true;
        self.condvar.notify_one();
    }

    fn wait(&self, timeout: Duration) {
        let woken = lock(&self.woken);
        let (mut woken, _) = self
            .condvar
            .wait_timeout_while(woken, timeout, |woken| !*woken)
            .unwrap_or_else(|e| e.into_inner());
        *woken = false;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The Core Audio objects of a tap, removed in the right order when dropped, also when setting up fails halfway.
#[derive(Default)]
struct Parts {
    tap: Option<AudioObjectID>,
    aggregate: Option<AudioObjectID>,
    io_proc: AudioDeviceIOProcID,
    started: bool,
    /// Whether the process list listener is registered.
    listening: bool,
    /// The client data of the IOProc and the listener, kept until both are gone: fields drop after `drop` ran.
    shared: Option<Arc<Shared>>,
}

impl Parts {
    fn client(&self) -> *mut c_void {
        self.shared
            .as_ref()
            .map_or(std::ptr::null_mut(), |shared| Arc::as_ptr(shared).cast_mut().cast())
    }
}

impl Drop for Parts {
    fn drop(&mut self) {
        // SAFETY: each part was made by the call that is undone here, and is undone once.
        unsafe {
            if self.listening {
                let address = process_list();
                AudioObjectRemovePropertyListener(
                    sys::SYSTEM,
                    NonNull::from(&address),
                    Some(processes_changed),
                    self.client(),
                );
            }
            if let Some(aggregate) = self.aggregate {
                if self.started {
                    AudioDeviceStop(aggregate, self.io_proc);
                }
                if self.io_proc.is_some() {
                    AudioDeviceDestroyIOProcID(aggregate, self.io_proc);
                }
                AudioHardwareDestroyAggregateDevice(aggregate);
            }
            if let Some(tap) = self.tap {
                AudioHardwareDestroyProcessTap(tap);
            }
        }
    }
}

fn process_list() -> AudioObjectPropertyAddress {
    sys::global(kAudioHardwarePropertyProcessObjectList)
}

/// The audio objects of the processes `follow` stands for, see [`members_of`].
fn members(follow: &Follow) -> Result<Vec<AudioObjectID>> {
    let (ids, processes) = sys::process_objects()?;
    Ok(members_of(&ids, &processes, follow))
}

/// The audio objects (`ids`, one per process) of the processes `follow` stands for, without asking for the apps' names
/// or paths: the watcher calls this twice a second.
fn members_of(ids: &[AudioObjectID], processes: &[processes::AudioProcess], follow: &Follow) -> Vec<AudioObjectID> {
    let mut members: Vec<AudioObjectID> = processes::members(processes, std::process::id(), follow)
        .into_iter()
        .map(|i| ids[i])
        .collect();
    // In a fixed order, so the watcher only changes the tap when the processes change.
    members.sort_unstable();
    members
}

impl Tap {
    /// Taps the app with PID `pid`, unmuted, and starts its IOProc.
    pub(crate) fn start(pid: u32) -> Result<Self> {
        let (ids, all) = sys::process_objects()?;
        let follow = processes::follow(&all, std::process::id(), pid).ok_or(CaptureError::AppNotFound(pid))?;
        let processes = members_of(&ids, &all, &follow);
        if processes.is_empty() {
            return Err(CaptureError::AppNotFound(pid));
        }
        let mut parts = Parts::default();
        let description = Description {
            uuid: new_uuid(),
            processes,
            mute: None,
        };
        let tap = description.create(&description.uuid, false)?;
        parts.tap = Some(tap);

        // Replays play where the app plays.
        let device = description
            .processes
            .iter()
            .find_map(|&process| sys::process_output_device(process))
            .or_else(sys::default_output_device)
            .ok_or_else(|| CaptureError::Backend("there is no output device".into()))?;
        let aggregate = create_aggregate(&description.uuid, &*sys::device_uid(device)?)?;
        parts.aggregate = Some(aggregate);

        // SAFETY: the tap's format is a stream description.
        let tap_format: AudioStreamBasicDescription = unsafe { sys::get(tap, sys::global(kAudioTapPropertyFormat)) }?;
        if tap_format.mFormatID != kAudioFormatLinearPCM
            || tap_format.mFormatFlags & kAudioFormatFlagIsFloat == 0
            || tap_format.mBitsPerChannel != 32
            || tap_format.mChannelsPerFrame == 0
        {
            return Err(CaptureError::Backend(format!(
                "the tap delivers an unexpected format: {tap_format:?}"
            )));
        }
        // The aggregate runs at its main device's rate, and the tap is resampled to it.
        let rate = sys::sample_rate(aggregate).unwrap_or(tap_format.mSampleRate);
        let format = CaptureFormat {
            sample_rate: rate.round() as u32,
            channels: tap_format.mChannelsPerFrame as u16,
        };
        let shared = Arc::new(Shared {
            pid,
            follow,
            tap,
            device,
            format,
            command: AtomicU64::new(Command::SILENT.pack()),
            frames: AtomicU64::new(0),
            gain: AtomicU32::new(0f32.to_bits()),
            ahead: AtomicU64::new(0),
            io: Mutex::new(Io {
                player: Player::new(format.sample_rate),
                tapped: Vec::with_capacity(MAX_CYCLE * usize::from(format.channels)),
            }),
            sink: Mutex::new(None),
            sinks: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            wake: Wake::default(),
            description: Mutex::new(description),
        });

        // The IOProc and the listener get `shared` as their client data; `parts` keeps it until they are gone.
        parts.shared = Some(Arc::clone(&shared));
        // SAFETY: the client data outlives the IOProc, see above.
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                aggregate,
                Some(io_proc),
                parts.client(),
                NonNull::from(&mut parts.io_proc),
            )
        };
        sys::check("AudioDeviceCreateIOProcID", status)?;
        use_only_the_tap(aggregate, parts.io_proc);
        // SAFETY: the IOProc was just made on this device.
        sys::check("AudioDeviceStart", unsafe {
            AudioDeviceStart(aggregate, parts.io_proc)
        })?;
        parts.started = true;
        // Like the other backends, return once audio flows.
        let started = Instant::now();
        while shared.frames.load(Ordering::Acquire) == 0 {
            if started.elapsed() > START_TIMEOUT {
                return Err(CaptureError::Backend("the app's output device doesn't run".into()));
            }
            thread::sleep(Duration::from_millis(5));
        }
        if cfg!(debug_assertions) {
            eprintln!(
                "tap of PID {pid}: {} Hz, {} channels, device {device}, {} frames ahead, latency in {} out {} frames, \
                 input streams {}",
                format.sample_rate,
                format.channels,
                shared.ahead.load(Ordering::Relaxed),
                sys::latency(aggregate, true),
                sys::latency(aggregate, false),
                sys::input_streams(aggregate)
            );
        }

        // New helper processes join the tap. A listener wakes the watcher when the list changes; it also looks every
        // `WATCH_EVERY` in case the listener can't be registered.
        let address = process_list();
        // SAFETY: as for the IOProc.
        let status = unsafe {
            AudioObjectAddPropertyListener(
                sys::SYSTEM,
                NonNull::from(&address),
                Some(processes_changed),
                parts.client(),
            )
        };
        parts.listening = status == 0;
        let watched = Arc::clone(&shared);
        let watcher = thread::Builder::new()
            .name(format!("tap watcher {pid}"))
            .spawn(move || watch(&watched))
            .map_err(|e| CaptureError::Backend(format!("could not start the tap's watcher: {e}")))?;
        Ok(Self {
            _parts: parts,
            shared,
            watcher: Some(watcher),
            sequencer: Mutex::new(Sequencer::new(format.sample_rate)),
        })
    }

    /// Whether the tap ended by itself, see [`Shared::ended`].
    pub(crate) fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    fn io_status(&self) -> IoStatus {
        let frames = self.shared.frames.load(Ordering::Acquire);
        IoStatus {
            frames,
            gain: f32::from_bits(self.shared.gain.load(Ordering::Relaxed)),
            latency: self.shared.ahead.load(Ordering::Relaxed),
        }
    }

    fn set_muted(&self, muted: bool) -> Result<()> {
        lock(&self.shared.description).set_muted(muted, self.shared.device)
    }

    /// Ducks the app to `level` (0–1, 1 restores it) and returns once it's there, see `passthrough`. If the IOProc
    /// doesn't run, the replay can't be heard, so the app is only muted while ducked. If the app can't be muted, it is
    /// left playing as it did, without a replay, and the error is returned.
    pub(crate) fn duck(&self, level: f32) -> Result<()> {
        let mut sequencer = lock(&self.sequencer);
        sequencer.set_target(level);
        let start = Instant::now();
        let (mut frames, mut moved) = (self.io_status().frames, Instant::now());
        loop {
            let io = self.io_status();
            if io.frames != frames {
                (frames, moved) = (io.frames, Instant::now());
            }
            if self.ended() || moved.elapsed() > DUCK_STALLED || start.elapsed() > DUCK_TIMEOUT {
                self.shared.command.store(Command::SILENT.pack(), Ordering::Relaxed);
                let muted = level < 1.0;
                if let Err(error) = self.set_muted(muted) {
                    return self.mute_failed(&mut sequencer, error);
                }
                sequencer.set_muted(muted, io);
                return Ok(());
            }
            match sequencer.next(io) {
                Step::Mute(muted) => {
                    if let Err(error) = self.set_muted(muted) {
                        return self.mute_failed(&mut sequencer, error);
                    }
                    sequencer.set_muted(muted, self.io_status());
                }
                Step::Wait(command) => {
                    self.shared.command.store(command.pack(), Ordering::Relaxed);
                    thread::sleep(DUCK_POLL);
                }
                Step::Done(command) => {
                    self.shared.command.store(command.pack(), Ordering::Relaxed);
                    return Ok(());
                }
            }
        }
    }

    /// Muting the app failed with `error`: it plays unmuted, so the replay goes out instead of doubling it, and the app
    /// isn't ducked.
    fn mute_failed(&self, sequencer: &mut Sequencer, error: CaptureError) -> Result<()> {
        self.shared
            .command
            .store(sequencer.mute_failed().pack(), Ordering::Relaxed);
        Err(error)
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.wake();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        let _ = self.set_muted(false);
        // `_parts` goes next.
    }
}

/// Creates a private aggregate device on the device with UID `device_uid`, with the tap `tap_uuid` as its input.
fn create_aggregate(tap_uuid: &str, device_uid: &CFString) -> Result<AudioObjectID> {
    let key = |key: &CStr| CFString::from_str(key.to_str().unwrap_or_default());
    let yes: &CFType = CFBoolean::new(true);
    let no: &CFType = CFBoolean::new(false);

    let sub_device = CFDictionary::<CFString, CFType>::from_slices(&[&*key(kAudioSubDeviceUIDKey)], &[device_uid]);
    let tap_uuid = CFString::from_str(tap_uuid);
    let sub_tap = CFDictionary::<CFString, CFType>::from_slices(
        &[&*key(kAudioSubTapUIDKey), &*key(kAudioSubTapDriftCompensationKey)],
        &[&*tap_uuid, yes],
    );
    let sub_devices = CFArray::from_retained_objects(&[sub_device]);
    let taps = CFArray::from_retained_objects(&[sub_tap]);
    let name = CFString::from_static_str("Repeat That");
    let uid = CFString::from_str(&autoreleasepool(|_| NSUUID::new().UUIDString().to_string()));

    let keys = [
        key(kAudioAggregateDeviceNameKey),
        key(kAudioAggregateDeviceUIDKey),
        key(kAudioAggregateDeviceMainSubDeviceKey),
        key(kAudioAggregateDeviceIsPrivateKey),
        key(kAudioAggregateDeviceIsStackedKey),
        key(kAudioAggregateDeviceTapAutoStartKey),
        key(kAudioAggregateDeviceSubDeviceListKey),
        key(kAudioAggregateDeviceTapListKey),
    ];
    let values: [&CFType; 8] = [&name, &uid, device_uid, yes, no, yes, &sub_devices, &taps];
    let keys: Vec<&CFString> = keys.iter().map(|k| &**k).collect();
    let description = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    let mut aggregate = 0;
    // SAFETY: a description with the keys Core Audio documents, and a place for the ID.
    let status = unsafe { AudioHardwareCreateAggregateDevice(description.as_opaque(), NonNull::from(&mut aggregate)) };
    sys::check("AudioHardwareCreateAggregateDevice", status)?;
    Ok(aggregate)
}

/// Turns off the aggregate's inputs other than the tap (the last one) for our IOProc: the app's device may have a
/// microphone, which isn't ours to record. If that fails, the IOProc skips them itself.
fn use_only_the_tap(aggregate: AudioObjectID, io_proc: AudioDeviceIOProcID) {
    let streams = sys::input_streams(aggregate);
    let Some(proc_pointer) = io_proc.map(|f| f as *mut c_void).and_then(NonNull::new) else {
        return;
    };
    if streams < 2 {
        return;
    }
    // The struct ends in an array as long as there are streams.
    let header = std::mem::offset_of!(AudioHardwareIOProcStreamUsage, mStreamIsOn);
    let words = (header + streams * 4).div_ceil(8);
    let mut usage = vec![0u64; words];
    let base = usage.as_mut_ptr().cast::<u8>();
    // SAFETY: `usage` is big enough and 8-byte aligned for the header and `streams` flags.
    unsafe {
        let header_ptr = base.cast::<AudioHardwareIOProcStreamUsage>();
        (&raw mut (*header_ptr).mIOProc).write(proc_pointer);
        (&raw mut (*header_ptr).mNumberStreams).write(streams as u32);
        let flags = base.add(header).cast::<u32>();
        for stream in 0..streams {
            flags.add(stream).write(u32::from(stream + 1 == streams));
        }
        let address = sys::address(kAudioDevicePropertyIOProcStreamUsage, kAudioObjectPropertyScopeInput);
        let _ = objc2_core_audio::AudioObjectSetPropertyData(
            aggregate,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            (header + streams * 4) as u32,
            NonNull::new_unchecked(base.cast()),
        );
    }
}

/// The IOProc of the muting tap's aggregate device: it only has to run for the tap to mute, and plays silence.
unsafe extern "C-unwind" fn silence(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    _input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    _client: *mut c_void,
) -> i32 {
    // SAFETY: the cycle's output buffers, `mDataByteSize` bytes each (or none).
    for buffer in unsafe { buffers(output) } {
        if !buffer.mData.is_null() {
            unsafe { std::ptr::write_bytes(buffer.mData.cast::<u8>(), 0, buffer.mDataByteSize as usize) };
        }
    }
    0
}

/// Core Audio's process list changed: wake the watcher.
unsafe extern "C-unwind" fn processes_changed(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: the client data is the tap's `Shared`, which outlives the listener.
    let shared = unsafe { &*client.cast::<Shared>() };
    shared.wake.wake();
    0
}

/// The IOProc: hands the tapped audio to the capture and plays the replay. Runs on Core Audio's real-time thread, so
/// it never blocks and never allocates (unless a cycle is longer than `MAX_CYCLE`).
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: the client data is the tap's `Shared`, which outlives the IOProc.
    let shared = unsafe { &*client.cast::<Shared>() };
    // SAFETY: Core Audio's timestamps for this cycle.
    let (input_time, output_time) = unsafe { (input_time.as_ref(), output_time.as_ref()) };
    let valid = |time: &AudioTimeStamp| time.mFlags.contains(AudioTimeStampFlags::SampleTimeValid);
    if valid(input_time) && valid(output_time) {
        let ahead = (output_time.mSampleTime - input_time.mSampleTime).max(0.0);
        shared.ahead.store(ahead.round() as u64, Ordering::Relaxed);
    }
    // A panic must not unwind into Core Audio.
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Core Audio's buffer lists for this cycle.
        unsafe { shared.cycle(input, output) }
    }));
    if result.is_err() {
        shared.ended.store(true, Ordering::Relaxed);
    }
    0
}

/// The buffers of a Core Audio buffer list.
///
/// # Safety
///
/// `list` must be a valid buffer list for the duration of the returned slice.
unsafe fn buffers<'a>(list: NonNull<AudioBufferList>) -> &'a [AudioBuffer] {
    // SAFETY: the list holds `mNumberBuffers` buffers, starting at `mBuffers`.
    unsafe {
        let list = list.as_ptr();
        let count = (*list).mNumberBuffers as usize;
        std::slice::from_raw_parts((&raw const (*list).mBuffers).cast::<AudioBuffer>(), count)
    }
}

impl Shared {
    /// One IO cycle.
    ///
    /// # Safety
    ///
    /// `input` and `output` must be the cycle's buffer lists.
    unsafe fn cycle(&self, input: NonNull<AudioBufferList>, output: NonNull<AudioBufferList>) {
        let mut io = match self.io.try_lock() {
            Ok(io) => io,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        let io = &mut *io;
        let channels = usize::from(self.format.channels);

        // SAFETY: the cycle's buffers, each `mDataByteSize` bytes of f32 samples (or none).
        let inputs = unsafe { buffers(input) };
        let mut tapped = [Buffer {
            channels: 0,
            samples: &[],
        }; MAX_BUFFERS];
        let count = inputs.len().min(MAX_BUFFERS);
        for (slot, buffer) in tapped.iter_mut().zip(inputs) {
            *slot = Buffer {
                channels: buffer.mNumberChannels as usize,
                samples: if buffer.mData.is_null() {
                    &[]
                } else {
                    // SAFETY: see above.
                    unsafe { std::slice::from_raw_parts(buffer.mData.cast::<f32>(), buffer.mDataByteSize as usize / 4) }
                },
            };
        }
        let frames = passthrough::interleave(&tapped[..count], channels, &mut io.tapped);
        if frames > 0 {
            match self.sink.try_lock() {
                Ok(mut sink) => {
                    if let Some((_, sink)) = sink.as_mut() {
                        sink(&io.tapped, self.format);
                    }
                }
                Err(TryLockError::Poisoned(e)) => {
                    if let Some((_, sink)) = e.into_inner().as_mut() {
                        sink(&io.tapped, self.format);
                    }
                }
                // A capture is being attached or detached; this cycle goes to neither.
                Err(TryLockError::WouldBlock) => {}
            }
        }

        // SAFETY: as for the input, and ours to write.
        let outputs = unsafe { buffers(output) };
        let mut out: [BufferMut; MAX_BUFFERS] = std::array::from_fn(|_| BufferMut {
            channels: 0,
            samples: &mut [],
        });
        let count = outputs.len().min(MAX_BUFFERS);
        for (slot, buffer) in out.iter_mut().zip(outputs) {
            if !buffer.mData.is_null() {
                *slot = BufferMut {
                    channels: buffer.mNumberChannels as usize,
                    // SAFETY: see above.
                    samples: unsafe {
                        std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), buffer.mDataByteSize as usize / 4)
                    },
                };
            }
        }
        let command = Command::unpack(self.command.load(Ordering::Relaxed));
        let played = passthrough::play(&io.tapped, channels, &mut io.player, command, &mut out[..count]);
        self.gain.store(io.player.gain().to_bits(), Ordering::Relaxed);
        self.frames.fetch_add(played as u64, Ordering::Release);
    }
}

/// The watcher thread: follows the app's processes, and ends the tap when it can't go on.
fn watch(shared: &Shared) {
    let mut checked_permission = Instant::now();
    let (mut frames, mut moved) = (0, Instant::now());
    while !shared.stop.load(Ordering::Relaxed) {
        shared.wake.wait(WATCH_EVERY);
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let ended = |why: &str| {
            if cfg!(debug_assertions) {
                eprintln!("tap of PID {} ended: {why}", shared.pid);
            }
            shared.ended.store(true, Ordering::Relaxed);
        };
        match members(&shared.follow) {
            Ok(processes) if processes.is_empty() => return ended("the app quit"),
            Ok(processes) => {
                let mut description = lock(&shared.description);
                if description.processes != processes {
                    let old = std::mem::replace(&mut description.processes, processes);
                    let applied = description.apply(shared.tap);
                    if cfg!(debug_assertions) {
                        eprintln!(
                            "tap of PID {}: processes {old:?} -> {:?}: {applied:?}",
                            shared.pid, description.processes
                        );
                    }
                    if applied.is_err() {
                        description.processes = old;
                    }
                }
            }
            // Core Audio is restarting, or busy: next time.
            Err(_) => {}
        }
        if !sys::device_alive(shared.device) {
            return ended("its device went away");
        }
        let now = shared.frames.load(Ordering::Relaxed);
        if now != frames {
            (frames, moved) = (now, Instant::now());
        } else if moved.elapsed() > IO_STALLED {
            return ended("its device stopped");
        }
        if checked_permission.elapsed() >= PERMISSION_EVERY {
            checked_permission = Instant::now();
            if permission::status().is_ok_and(|status| status == PermissionStatus::Denied) {
                return ended("capture isn't allowed any more");
            }
        }
    }
}

/// A capture of a tap: its sink gets the tapped audio until this is dropped.
pub(crate) struct Capture {
    tap: Arc<Tap>,
    id: u64,
}

impl Capture {
    pub(crate) fn new(tap: Arc<Tap>, sink: FrameSink) -> Self {
        let id = tap.shared.sinks.fetch_add(1, Ordering::Relaxed);
        *lock(&tap.shared.sink) = Some((id, sink));
        Self { tap, id }
    }

    fn attached(&self) -> bool {
        lock(&self.tap.shared.sink)
            .as_ref()
            .is_some_and(|(id, _)| *id == self.id)
    }
}

impl CaptureHandle for Capture {
    fn is_running(&self) -> bool {
        !self.tap.ended() && self.attached()
    }

    fn stop(self: Box<Self>) {
        // Drop does the work.
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let mut sink = lock(&self.tap.shared.sink);
        if sink.as_ref().is_some_and(|(id, _)| *id == self.id) {
            *sink = None;
        }
    }
}
