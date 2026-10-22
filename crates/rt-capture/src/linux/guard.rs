//! The duck guard on Linux: the app's own executable, started again with the hidden guard arguments, waits for the app
//! to exit and restores what the duck journal still lists. In a Flatpak there is no guard, because it would die with
//! the sandbox; the journal alone restores ducked apps there, at the next start.

use std::ffi::OsString;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use std::{env, thread};

use crate::duck_guard;

/// Whether this process runs in a Flatpak sandbox: Flatpak sets `FLATPAK_ID`, and `/.flatpak-info` is in every sandbox
/// whatever the environment. Looked at once, as the answer can't change while the process runs.
pub fn in_flatpak() -> bool {
    static IN_FLATPAK: OnceLock<bool> = OnceLock::new();
    *IN_FLATPAK.get_or_init(|| sandboxed_by(env::var_os("FLATPAK_ID"), Path::new("/.flatpak-info").exists()))
}

fn sandboxed_by(flatpak_id: Option<OsString>, flatpak_info: bool) -> bool {
    flatpak_id.is_some_and(|id| !id.is_empty()) || flatpak_info
}

/// The program that runs the guard: the AppImage itself when the app runs from one, because the AppImage's mount under
/// `/tmp/.mount_*`, where the running executable is, goes away when the app exits.
fn program(appimage: Option<OsString>, current_exe: io::Result<PathBuf>) -> io::Result<PathBuf> {
    match appimage.filter(|path| !path.is_empty()) {
        Some(appimage) => Ok(appimage.into()),
        None => current_exe,
    }
}

/// Starts the guard for `journal`, waiting for this process. It runs in a process group of its own, so a Ctrl+C in the
/// terminal the app was started from doesn't take it down with the app.
pub(super) fn spawn(journal: &Path) -> io::Result<()> {
    let mut child = Command::new(program(env::var_os("APPIMAGE"), env::current_exe())?)
        .args(duck_guard::args(journal, std::process::id()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // The guard normally outlives us. If it ends first, this collects it rather than leaving a zombie.
    thread::Builder::new()
        .name("rt-duck-guard-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}

/// Waits until process `pid` has exited.
pub(super) fn wait_for_exit(pid: u32) {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return;
    };
    // A pidfd turns readable when the process exits (Linux 5.3).
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, raw, 0) };
    if fd >= 0 {
        let fd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
        let mut poll = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&fd),
            events: libc::POLLIN,
            revents: 0,
        };
        while unsafe { libc::poll(&mut poll, 1, -1) } < 0
            && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
        {}
        return;
    }
    if io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return;
    }
    // An older kernel: look every 100 ms.
    while alive(raw) {
        thread::sleep(Duration::from_millis(100));
    }
}

/// Whether process `pid` exists (also when it belongs to someone else).
pub(super) fn alive(pid: libc::pid_t) -> bool {
    let signalled = unsafe { libc::kill(pid, 0) } == 0;
    signalled || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Instant;

    use super::*;

    #[test]
    fn the_guard_runs_from_the_appimage_when_there_is_one() {
        let mounted = || Ok(PathBuf::from("/tmp/.mount_RepeatXYZ/usr/bin/repeat-that"));
        assert_eq!(
            program(Some("/home/me/Repeat-That.AppImage".into()), mounted()).unwrap(),
            Path::new("/home/me/Repeat-That.AppImage")
        );
        assert_eq!(
            program(None, mounted()).unwrap(),
            Path::new("/tmp/.mount_RepeatXYZ/usr/bin/repeat-that")
        );
        assert_eq!(
            program(Some("".into()), mounted()).unwrap(),
            Path::new("/tmp/.mount_RepeatXYZ/usr/bin/repeat-that")
        );
    }

    #[test]
    fn a_flatpak_is_recognized_by_its_id_or_its_info_file() {
        assert!(sandboxed_by(Some("me.roboroads.RepeatThat".into()), false));
        assert!(sandboxed_by(None, true));
        assert!(!sandboxed_by(None, false));
        assert!(!sandboxed_by(Some("".into()), false));
    }

    #[test]
    fn waits_for_a_process_to_exit() {
        let mut child = Command::new("sleep").arg("0.3").spawn().expect("run sleep");
        let pid = child.id();
        let start = Instant::now();
        // A thread collects it, as the guard's parent isn't ours to wait for.
        let reaper = thread::spawn(move || child.wait());
        assert!(alive(pid as libc::pid_t));
        wait_for_exit(pid);
        assert!(start.elapsed() >= Duration::from_millis(250), "{:?}", start.elapsed());
        reaper.join().unwrap().unwrap();
        // A process that is already gone.
        wait_for_exit(pid);
    }
}
