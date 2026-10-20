//! The duck thread: lowers the captured app's volume while a replay plays, and restores it afterwards.
//!
//! [`AppAudioSource::set_app_volume`] fades and blocks for a few hundred milliseconds, so it runs here, away from the
//! player. The latest level asked for wins: a level asked for during a fade applies after it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use rt_capture::{AppAudioSource, AudioApp, CaptureError};

/// How long after a replay ends the volume comes back. A new replay within it keeps the app ducked.
pub(crate) const RESTORE_DELAY: Duration = Duration::from_millis(300);
/// A volume change that failed is tried again after this long, until it works or the app is gone.
const RETRY_AFTER: Duration = Duration::from_millis(500);

pub(crate) enum DuckCmd {
    /// Duck to this level now, cancelling a restore that was due.
    Duck(f32),
    /// Restore the volume after [`RESTORE_DELAY`], unless a [`DuckCmd::Duck`] comes first.
    Restore,
    /// The app to duck from now on. The old one gets its volume back first.
    SetApp(Option<AudioApp>),
    /// Restore the volume and end.
    Shutdown,
}

pub(crate) struct Duck {
    source: Arc<dyn AppAudioSource>,
    app: Option<AudioApp>,
    /// The level the app is at.
    applied: f32,
    /// The level it should be at.
    wanted: f32,
    restore_at: Option<Instant>,
    /// The last volume change failed: try again then.
    retry_at: Option<Instant>,
}

impl Duck {
    pub(crate) fn new(source: Arc<dyn AppAudioSource>) -> Self {
        Self {
            source,
            app: None,
            applied: 1.0,
            wanted: 1.0,
            restore_at: None,
            retry_at: None,
        }
    }

    pub(crate) fn run(mut self, commands: Receiver<DuckCmd>) {
        loop {
            let deadline = match (self.restore_at, self.retry_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let command = match deadline {
                Some(at) => commands.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match command {
                Ok(DuckCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => {}
            }
            // Everything that came in meanwhile, so only the latest level is applied.
            while let Ok(command) = commands.try_recv() {
                if matches!(command, DuckCmd::Shutdown) {
                    self.wanted = 1.0;
                    self.apply();
                    return;
                }
                self.handle(command);
            }
            if self.restore_at.is_some_and(|at| at <= Instant::now()) {
                self.restore_at = None;
                self.wanted = 1.0;
            }
            self.apply();
        }
        self.wanted = 1.0;
        self.apply();
    }

    fn handle(&mut self, command: DuckCmd) {
        match command {
            DuckCmd::Duck(level) => {
                self.wanted = level;
                self.restore_at = None;
            }
            DuckCmd::Restore => self.restore_at = Some(Instant::now() + RESTORE_DELAY),
            DuckCmd::SetApp(app) => {
                if app != self.app {
                    // The old app gets its own volume back, in one try: it's no longer ours to retry.
                    let wanted = std::mem::replace(&mut self.wanted, 1.0);
                    self.apply();
                    self.app = app;
                    self.applied = 1.0;
                    self.retry_at = None;
                    if self.app.is_some() {
                        // The target is back with a new PID (wipes clear the app first): ducked like the old one.
                        self.wanted = wanted;
                    } else {
                        self.restore_at = None;
                    }
                }
            }
            DuckCmd::Shutdown => {}
        }
    }

    fn apply(&mut self) {
        if self.wanted == self.applied {
            self.retry_at = None;
            return;
        }
        if let Some(app) = &self.app {
            match self.source.set_app_volume(app, self.wanted) {
                // An app that's gone (it quit, or restarted with a new PID) has nothing to restore.
                Ok(()) | Err(CaptureError::AppNotFound(_)) => {}
                Err(_) => {
                    self.retry_at = Some(Instant::now() + RETRY_AFTER);
                    return;
                }
            }
        }
        self.applied = self.wanted;
        self.retry_at = None;
    }
}
