//! Audio sessions: which apps are playing audio, and their per-app volume (`ISimpleAudioVolume`) for ducking.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, HANDLE, S_OK};
use windows::Win32::Media::Audio::{
    AudioSessionStateExpired, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, eRender,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{HSTRING, Interface, PWSTR, w};

use super::backend;
use crate::process_tree::{Process, ProcessTable};
use crate::{AudioApp, Result};

/// Apps with an audio session on any active output device, one entry per process tree, sorted by name.
pub(super) fn list_apps() -> Result<Vec<AudioApp>> {
    let processes = process_table()?;
    let own_root = processes.app_root(std::process::id());
    let mut roots = Vec::new();
    for_each_session(|session| {
        let pid = unsafe { session.GetProcessId() }.unwrap_or(0);
        let root = processes.app_root(pid);
        if pid != 0 && root != own_root && !roots.contains(&root) {
            roots.push(root);
        }
        Ok(())
    })?;

    let mut apps: Vec<AudioApp> = roots
        .into_iter()
        .map(|pid| {
            let exe_path = exe_path(pid);
            let name = exe_path
                .as_deref()
                .and_then(file_description)
                .or_else(|| {
                    exe_path
                        .as_deref()
                        .and_then(Path::file_stem)
                        .map(|s| s.to_string_lossy().into_owned())
                })
                .or_else(|| processes.0.get(&pid).map(|p| p.exe.trim_end_matches(".exe").to_owned()))
                .unwrap_or_else(|| format!("PID {pid}"));
            AudioApp { pid, name, exe_path }
        })
        .collect();
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    Ok(apps)
}

/// Sets every audio session in `root`'s process tree to `level` times its volume from before ducking. At `1.0` the
/// sessions get their old volume back and are forgotten.
pub(super) fn duck(root: u32, level: f32, before_duck: &mut HashMap<String, f32>) -> Result<()> {
    let processes = process_table()?;
    for_each_session(|session| {
        let pid = unsafe { session.GetProcessId() }.unwrap_or(0);
        if pid == 0 || !processes.is_in_tree(pid, root) {
            return Ok(());
        }
        let id = take_co_string(unsafe { session.GetSessionInstanceIdentifier() }.map_err(backend("session id"))?);
        let volume: ISimpleAudioVolume = session.cast().map_err(backend("ISimpleAudioVolume"))?;
        let target = if level >= 1.0 {
            match before_duck.remove(&id) {
                Some(original) => original,
                None => return Ok(()),
            }
        } else {
            let current = unsafe { volume.GetMasterVolume() }.map_err(backend("GetMasterVolume"))?;
            *before_duck.entry(id).or_insert(current) * level
        };
        unsafe { volume.SetMasterVolume(target, std::ptr::null()) }.map_err(backend("SetMasterVolume"))
    })
}

/// Calls `f` for every live, non-system audio session on every active output device.
fn for_each_session(mut f: impl FnMut(&IAudioSessionControl2) -> Result<()>) -> Result<()> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(backend("MMDeviceEnumerator"))?;
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(backend("EnumAudioEndpoints"))?;
        for d in 0..devices.GetCount().map_err(backend("device count"))? {
            // A device can disappear while we look at it; skip it rather than failing the whole list.
            let Ok(device) = devices.Item(d) else { continue };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            for s in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(session) = sessions.GetSession(s).and_then(|s| s.cast::<IAudioSessionControl2>()) else {
                    continue;
                };
                if session.IsSystemSoundsSession() == S_OK
                    || session.GetState().is_ok_and(|state| state == AudioSessionStateExpired)
                {
                    continue;
                }
                f(&session)?;
            }
        }
    }
    Ok(())
}

/// A snapshot of all processes with their parent and executable name.
fn process_table() -> Result<ProcessTable> {
    let mut table = HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(backend("CreateToolhelp32Snapshot"))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = Process32FirstW(snapshot, &mut entry);
        while found.is_ok() {
            table.insert(
                entry.th32ProcessID,
                Process {
                    parent: entry.th32ParentProcessID,
                    exe: wide_to_string(&entry.szExeFile),
                },
            );
            found = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
    }
    Ok(ProcessTable(table))
}

/// Full path of `pid`'s executable, if we may see it.
fn exe_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let path = query_image_name(process);
        let _ = CloseHandle(process);
        path
    }
}

unsafe fn query_image_name(process: HANDLE) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// The "File description" from an executable's version info, e.g. "Microsoft Teams".
fn file_description(exe: &Path) -> Option<String> {
    let path = HSTRING::from(exe.as_os_str());
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()).ok()?;

        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(
            data.as_ptr().cast(),
            w!("\\VarFileInfo\\Translation"),
            &mut ptr,
            &mut len,
        )
        .as_bool()
            || len < 4
        {
            return None;
        }
        let lang = *ptr.cast::<u16>();
        let codepage = *ptr.cast::<u16>().add(1);
        let key = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"));
        if !VerQueryValueW(data.as_ptr().cast(), &key, &mut ptr, &mut len).as_bool() || len == 0 {
            return None;
        }
        let text = wide_to_string(std::slice::from_raw_parts(ptr.cast::<u16>(), len as usize));
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

/// A string from a NUL-terminated (or full) UTF-16 buffer.
fn wide_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

/// Copies a string that COM allocated for us, then frees it.
fn take_co_string(s: PWSTR) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe {
        let text = s.to_string().unwrap_or_default();
        CoTaskMemFree(Some(s.0 as *const c_void));
        text
    }
}
