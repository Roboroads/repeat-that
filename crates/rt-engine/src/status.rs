//! What the engine is doing, for the tray and the settings window.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    pub capture: CaptureState,
    /// Name of the target app, also while it isn't running.
    pub target_name: Option<String>,
    pub playback: PlaybackState,
    /// How long the replay takes to reach live if nothing more is said. `None` while idle.
    pub behind: Option<Duration>,
    /// Speech in the buffer.
    pub buffered: Duration,
    pub output: OutputStatus,
    /// Silero VAD couldn't load, so the less accurate energy VAD finds speech.
    pub vad_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CaptureState {
    #[default]
    NoTarget,
    /// The target app isn't running (or plays no audio); polled every 2 s.
    Waiting,
    Capturing,
    Paused,
    /// Capture failed to start; retried every 2 s.
    Error(String),
    /// Capturing isn't possible on this system; not retried.
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    #[default]
    Idle,
    Replaying,
    CatchingUp,
}

/// The output of the latest replay (or beep).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutputStatus {
    pub device_name: Option<String>,
    /// The chosen device is missing or failed, so the default device played instead.
    pub fell_back: bool,
    /// No device could play: the replay was aborted.
    pub error: Option<String>,
}

/// How many statuses a subscriber can fall behind before it is dropped: half a minute of a replay's updates.
pub(crate) const SUBSCRIBER_BACKLOG: usize = 64;

/// The current status, shared by the engine's threads, and everyone who wants to hear about changes.
#[derive(Default)]
pub(crate) struct StatusHub {
    current: Mutex<Status>,
    subscribers: Mutex<Vec<SyncSender<Status>>>,
}

impl StatusHub {
    pub(crate) fn get(&self) -> Status {
        self.lock().clone()
    }

    pub(crate) fn subscribe(&self) -> Receiver<Status> {
        let (tx, rx) = mpsc::sync_channel(SUBSCRIBER_BACKLOG);
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        // Sent while holding the subscriber list, so no change can come in between.
        let _ = tx.try_send(self.get());
        subscribers.push(tx);
        rx
    }

    /// Changes the status with `change`, and tells every subscriber if that changed anything. Returns whether it did.
    pub(crate) fn update(&self, change: impl FnOnce(&mut Status)) -> bool {
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        let status = {
            let mut current = self.lock();
            let before = current.clone();
            change(&mut current);
            if *current == before {
                return false;
            }
            current.clone()
        };
        send(&mut subscribers, &status);
        true
    }

    /// Sends the status to every subscriber, changed or not.
    pub(crate) fn broadcast(&self) {
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        let status = self.get();
        send(&mut subscribers, &status);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.current.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Sends `status` to every subscriber. One that went away, or fell [`SUBSCRIBER_BACKLOG`] behind, is dropped, which
/// ends its channel; it can subscribe again.
fn send(subscribers: &mut Vec<SyncSender<Status>>, status: &Status) {
    subscribers.retain(|tx| tx.try_send(status.clone()).is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subscriber_that_stops_reading_is_dropped_once_far_behind() {
        let hub = StatusHub::default();
        let rx = hub.subscribe();
        for ms in 0..1000 {
            hub.update(|s| s.buffered = Duration::from_millis(ms));
        }
        assert!(hub.subscribers.lock().unwrap().is_empty());
        // What it got stays bounded, then its channel ends.
        assert_eq!(rx.iter().count(), SUBSCRIBER_BACKLOG);
    }

    #[test]
    fn subscribers_get_the_current_status_then_changes_only() {
        let hub = StatusHub::default();
        hub.update(|s| s.capture = CaptureState::Waiting);
        let rx = hub.subscribe();
        assert_eq!(rx.try_recv().unwrap().capture, CaptureState::Waiting);

        hub.update(|s| s.capture = CaptureState::Waiting);
        assert!(rx.try_recv().is_err());
        hub.update(|s| s.playback = PlaybackState::Replaying);
        assert_eq!(rx.try_recv().unwrap().playback, PlaybackState::Replaying);
        hub.broadcast();
        assert_eq!(rx.try_recv().unwrap().playback, PlaybackState::Replaying);
    }

    #[test]
    fn forgets_subscribers_that_went_away() {
        let hub = StatusHub::default();
        drop(hub.subscribe());
        hub.update(|s| s.vad_fallback = true);
        assert!(hub.subscribers.lock().unwrap().is_empty());
    }
}
