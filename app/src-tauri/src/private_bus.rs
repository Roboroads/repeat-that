//! A D-Bus daemon of its own, for the tests of the code that talks to the real bus (the tray host check, the shortcuts
//! portal). Nothing in them touches the user's session, and nothing is written but the daemon's socket.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct PrivateBus {
    child: Child,
    dir: PathBuf,
    /// Where to connect, `unix:path=…`.
    pub address: String,
}

impl PrivateBus {
    /// Starts a daemon. `None` when `dbus-daemon` isn't installed, which a test takes as a reason to skip itself: CI has it,
    /// and so does anyone who runs the end-to-end tests.
    pub fn start() -> Option<Self> {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "repeat-that-bus-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("a folder for the bus socket");
        let socket = dir.join("bus");
        let address = format!("unix:path={}", socket.display());
        let child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", &format!("--address={address}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(child) = child else {
            eprintln!("skipped: dbus-daemon is not installed");
            let _ = std::fs::remove_dir_all(&dir);
            return None;
        };
        let bus = PrivateBus { child, dir, address };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "dbus-daemon did not open its socket");
            std::thread::sleep(Duration::from_millis(20));
        }
        Some(bus)
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
