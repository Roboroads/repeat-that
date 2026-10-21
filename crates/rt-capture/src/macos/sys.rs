//! Thin wrappers around the C functions the macOS backend uses: Core Audio's object properties, libproc, the
//! responsibility function and the OS version.

use std::ffi::{CStr, c_void};
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2::rc::autoreleasepool;
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertyScope, AudioObjectPropertySelector, AudioObjectSetPropertyData,
    kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertyStreams, kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyProcessObjectList,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioProcessPropertyDevices,
    kAudioProcessPropertyIsRunningOutput, kAudioProcessPropertyPID,
};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_foundation::{NSFileManager, NSProcessInfo, NSString};

use super::processes::AudioProcess;
use crate::{CaptureError, Result};

/// The system object, which lists processes and devices.
pub(crate) const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

pub(crate) fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

pub(crate) fn global(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    address(selector, kAudioObjectPropertyScopeGlobal)
}

/// A Core Audio status as a [`CaptureError`] that says which call failed. Statuses are often four letters, like `!obj`.
pub(crate) fn status_error(call: &str, status: i32) -> CaptureError {
    let bytes = status.to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        CaptureError::Backend(format!("{call}: '{}'", String::from_utf8_lossy(&bytes)))
    } else {
        CaptureError::Backend(format!("{call}: error {status}"))
    }
}

/// `Ok` for status 0.
pub(crate) fn check(call: &str, status: i32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(status_error(call, status))
    }
}

/// Reads a property that is one `T`.
///
/// # Safety
///
/// `T` must be the property's type, valid for any bit pattern Core Audio writes.
pub(crate) unsafe fn get<T: Copy>(object: AudioObjectID, address: AudioObjectPropertyAddress) -> Result<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    // SAFETY: room for one `T`, which the caller says the property is.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast()),
        )
    };
    check("AudioObjectGetPropertyData", status)?;
    if size as usize != size_of::<T>() {
        return Err(CaptureError::Backend(format!(
            "property '{}' has {size} bytes, expected {}",
            fourcc(address.mSelector),
            size_of::<T>()
        )));
    }
    // SAFETY: Core Audio wrote all of it.
    Ok(unsafe { value.assume_init() })
}

/// Reads a property that is an array of `T`.
///
/// # Safety
///
/// As for [`get`].
pub(crate) unsafe fn get_list<T: Copy + Default>(
    object: AudioObjectID,
    address: AudioObjectPropertyAddress,
) -> Result<Vec<T>> {
    let mut size = 0u32;
    // SAFETY: a valid address and a place for the size.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    check("AudioObjectGetPropertyDataSize", status)?;
    let mut list = vec![T::default(); size as usize / size_of::<T>()];
    if list.is_empty() {
        return Ok(list);
    }
    let mut size = (list.len() * size_of::<T>()) as u32;
    // SAFETY: room for `size` bytes of `T`s.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(list.as_mut_ptr().cast()),
        )
    };
    check("AudioObjectGetPropertyData", status)?;
    // The list may have shrunk in between.
    list.truncate(size as usize / size_of::<T>());
    Ok(list)
}

/// Sets a property to `value`.
///
/// # Safety
///
/// `T` must be the property's type.
pub(crate) unsafe fn set<T>(object: AudioObjectID, address: AudioObjectPropertyAddress, value: &T) -> Result<()> {
    // SAFETY: `value` is a `T`, which the caller says the property is.
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            NonNull::from(value).cast(),
        )
    };
    check("AudioObjectSetPropertyData", status)
}

fn fourcc(selector: u32) -> String {
    String::from_utf8_lossy(&selector.to_be_bytes()).into_owned()
}

/// The processes that use Core Audio, with their audio objects, in the same order.
pub(crate) fn process_objects() -> Result<(Vec<AudioObjectID>, Vec<AudioProcess>)> {
    // SAFETY: the process list is an array of object IDs.
    let objects: Vec<AudioObjectID> = unsafe { get_list(SYSTEM, global(kAudioHardwarePropertyProcessObjectList)) }?;
    let mut ids = Vec::with_capacity(objects.len());
    let mut processes = Vec::with_capacity(objects.len());
    for object in objects {
        // SAFETY: a pid_t. A process that went away meanwhile is skipped.
        let Ok(pid) = (unsafe { get::<i32>(object, global(kAudioProcessPropertyPID)) }) else {
            continue;
        };
        let Ok(pid) = u32::try_from(pid) else {
            continue;
        };
        // SAFETY: a UInt32 used as a boolean.
        let playing = unsafe { get::<u32>(object, global(kAudioProcessPropertyIsRunningOutput)) }.is_ok_and(|v| v != 0);
        ids.push(object);
        processes.push(AudioProcess {
            pid,
            path: process_path(pid),
            responsible: responsible_pid(pid),
            playing,
        });
    }
    Ok((ids, processes))
}

/// [`process_objects`] without the objects.
pub(crate) fn audio_processes() -> Result<Vec<AudioProcess>> {
    Ok(process_objects()?.1)
}

/// The executable of process `pid`.
pub(crate) fn process_path(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: a buffer of the size given.
    let len = unsafe { libc::proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if len <= 0 {
        return None;
    }
    buffer.truncate(len as usize);
    Some(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
}

type Responsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;

/// The process macOS holds responsible for `pid`: the app for its helpers, or the terminal a tool runs in. Uses the
/// private `responsibility_get_pid_responsible_for_pid`, loaded at run time; `None` without it.
pub(crate) fn responsible_pid(pid: u32) -> Option<u32> {
    static FUNCTION: OnceLock<Option<Responsible>> = OnceLock::new();
    const NAME: &CStr = c"responsibility_get_pid_responsible_for_pid";
    let function = FUNCTION.get_or_init(|| {
        // SAFETY: looks up a symbol of libSystem, which has the type above where it exists.
        unsafe {
            let symbol = libc::dlsym(libc::RTLD_DEFAULT, NAME.as_ptr());
            (!symbol.is_null()).then(|| std::mem::transmute::<*mut c_void, Responsible>(symbol))
        }
    });
    // SAFETY: takes any PID and returns -1 for one it doesn't know.
    let responsible = unsafe { function.as_ref()?(pid as libc::pid_t) };
    u32::try_from(responsible).ok().filter(|&pid| pid > 0)
}

/// The name Finder shows for `path`: an app's name without `.app`, else the file name.
pub(crate) fn display_name(path: &Path) -> String {
    let Some(path) = path.to_str() else {
        return path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
    };
    autoreleasepool(|_| {
        NSFileManager::defaultManager()
            .displayNameAtPath(&NSString::from_str(path))
            .to_string()
    })
}

/// The macOS version as (major, minor).
pub(crate) fn os_version() -> (i64, i64) {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    (version.majorVersion as i64, version.minorVersion as i64)
}

/// The output device process object `process` plays to, if it plays to one.
pub(crate) fn process_output_device(process: AudioObjectID) -> Option<AudioObjectID> {
    // SAFETY: an array of device IDs.
    let devices: Vec<AudioObjectID> = unsafe {
        get_list(
            process,
            address(kAudioProcessPropertyDevices, kAudioObjectPropertyScopeOutput),
        )
    }
    .ok()?;
    devices.into_iter().find(|&d| d != 0)
}

/// The output device the system plays to by default.
pub(crate) fn default_output_device() -> Option<AudioObjectID> {
    // SAFETY: a device ID.
    unsafe { get::<AudioObjectID>(SYSTEM, global(kAudioHardwarePropertyDefaultOutputDevice)) }
        .ok()
        .filter(|&d| d != 0)
}

/// The unique ID of `device`, which aggregate devices refer to it by.
pub(crate) fn device_uid(device: AudioObjectID) -> Result<CFRetained<CFString>> {
    // SAFETY: a CFStringRef that the caller owns.
    let uid = unsafe { get::<*const CFString>(device, global(kAudioDevicePropertyDeviceUID)) }?;
    let uid = NonNull::new(uid.cast_mut()).ok_or_else(|| CaptureError::Backend("the device has no UID".into()))?;
    // SAFETY: Core Audio gave us a reference of our own.
    Ok(unsafe { CFRetained::from_raw(uid) })
}

/// Whether `device` still works (it wasn't unplugged).
pub(crate) fn device_alive(device: AudioObjectID) -> bool {
    // SAFETY: a UInt32 used as a boolean.
    unsafe { get::<u32>(device, global(kAudioDevicePropertyDeviceIsAlive)) }.is_ok_and(|alive| alive != 0)
}

/// The sample rate `device` runs at.
pub(crate) fn sample_rate(device: AudioObjectID) -> Option<f64> {
    // SAFETY: a Float64.
    unsafe { get::<f64>(device, global(kAudioDevicePropertyNominalSampleRate)) }
        .ok()
        .filter(|rate| *rate > 0.0)
}

/// How many input streams `device` has.
pub(crate) fn input_streams(device: AudioObjectID) -> usize {
    // SAFETY: an array of stream IDs.
    unsafe {
        get_list::<AudioObjectID>(
            device,
            address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput),
        )
    }
    .map_or(0, |streams| streams.len())
}
