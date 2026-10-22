//! The states of an update, as pure transitions on an [`UpdateStatus`]. Nothing here waits, asks the network or
//! publishes: the service in `updater.rs` calls these under its lock and sends whatever they leave behind.
//!
//! ```text
//!             begin_check                finish_check
//!   idle ──────────────────▶ checking ──────────────────▶ upToDate | available | error
//!                                                            │
//!                                       begin_download       ▼
//!                           downloading ◀───────────────── available
//!                                │  begin_install
//!                                ▼
//!                           installing        (download or install failing: error, with the update still named)
//! ```

use super::mode::UpdateMode;
use super::notes;
use super::source::Found;
use super::{AvailableUpdate, UpdateErrorKind, UpdateState, UpdateStatus};
use crate::ipc::CommandError;

/// What the status says when the release information can't be understood.
pub const UNREADABLE: &str = "Couldn't read the update information.";

pub struct Machine {
    status: UpdateStatus,
}

impl Machine {
    pub fn new(mode: UpdateMode, mode_reason: Option<String>, current_version: &str) -> Self {
        Machine {
            status: UpdateStatus {
                mode,
                mode_reason,
                current_version: current_version.to_string(),
                state: UpdateState::Idle,
                available: None,
                progress: None,
                error: None,
                error_kind: None,
                last_checked_at: None,
                manual: false,
            },
        }
    }

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    /// Starts a check. Returns `false`, and changes nothing, when there is no check to start: another check, a
    /// download or an install is running, or this install never looks for updates.
    pub fn begin_check(&mut self, manual: bool) -> bool {
        let looks = matches!(self.status.mode, UpdateMode::Install | UpdateMode::Notify);
        let busy = matches!(
            self.status.state,
            UpdateState::Checking | UpdateState::Downloading | UpdateState::Installing
        );
        if !looks || busy {
            return false;
        }
        // What was found stays until the answer is in, so the banner doesn't blink when the user checks again.
        self.status.state = UpdateState::Checking;
        self.status.error = None;
        self.status.error_kind = None;
        self.status.manual = manual;
        true
    }

    /// Ends the running check with what the source found, `Err` being what to tell the user. A version that isn't a
    /// plain `X.Y.Z` can't be linked to, so it counts as unreadable; one that isn't newer than the running version
    /// is no update. Does nothing when no check is running.
    pub fn finish_check(&mut self, outcome: Result<Option<Found>, String>, now_ms: u64) {
        if self.status.state != UpdateState::Checking {
            return;
        }
        self.status.last_checked_at = Some(now_ms);
        match outcome.and_then(|found| self.classify(found)) {
            Ok(Some(update)) => {
                self.status.state = UpdateState::Available;
                self.status.available = Some(update);
                self.status.error = None;
                self.status.error_kind = None;
            }
            Ok(None) => {
                self.status.state = UpdateState::UpToDate;
                self.status.available = None;
                self.status.error = None;
                self.status.error_kind = None;
            }
            Err(message) => {
                self.status.state = UpdateState::Error;
                self.status.available = None;
                self.status.error = Some(message);
                self.status.error_kind = Some(UpdateErrorKind::Check);
            }
        }
    }

    /// What `found` means for the running version.
    fn classify(&self, found: Option<Found>) -> Result<Option<AvailableUpdate>, String> {
        let Some(found) = found else { return Ok(None) };
        let Some(numbers) = notes::plain_version(&found.version) else {
            eprintln!(
                "the update server announced the version {:?}, which is not X.Y.Z",
                found.version
            );
            return Err(UNREADABLE.to_string());
        };
        if !notes::is_newer(&self.status.current_version, numbers) {
            return Ok(None);
        }
        let release_url = notes::release_url(&found.version).ok_or_else(|| UNREADABLE.to_string())?;
        Ok(Some(AvailableUpdate {
            version: found.version,
            date: found.date,
            notes: notes::clean_notes(&found.notes.unwrap_or_default()),
            release_url,
        }))
    }

    /// Starts the download of the update that was found, which only an install that can replace itself does.
    pub fn begin_download(&mut self) -> Result<(), CommandError> {
        if self.status.mode != UpdateMode::Install {
            return Err(CommandError::invalid("This version can't install updates itself"));
        }
        if self.status.state != UpdateState::Available {
            return Err(CommandError::invalid("There is no update to install"));
        }
        self.status.state = UpdateState::Downloading;
        self.status.progress = None;
        self.status.error = None;
        self.status.error_kind = None;
        Ok(())
    }

    /// How far the download is, `None` when its size isn't known. Only a running download has one.
    pub fn set_progress(&mut self, progress: Option<f64>) {
        if self.status.state == UpdateState::Downloading {
            self.status.progress = progress.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0));
        }
    }

    /// The download is done and the install begins.
    pub fn begin_install(&mut self) {
        if self.status.state == UpdateState::Downloading {
            self.status.state = UpdateState::Installing;
            self.status.progress = None;
        }
    }

    /// The download or the install failed: the update stays named, so the window can link to its page.
    pub fn fail_install(&mut self, message: String) {
        self.status.state = UpdateState::Error;
        self.status.progress = None;
        self.status.error = Some(message);
        self.status.error_kind = Some(UpdateErrorKind::Install);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::CommandErrorKind;

    const NOW: u64 = 1_700_000_000_000;

    fn machine(mode: UpdateMode) -> Machine {
        Machine::new(mode, None, "0.1.0")
    }

    fn found(version: &str) -> Found {
        Found {
            version: version.into(),
            date: Some("2026-10-22".into()),
            notes: Some("What's new\n<!-- marker -->".into()),
        }
    }

    /// An installing-capable machine whose check found `0.2.0`.
    fn available() -> Machine {
        let mut machine = machine(UpdateMode::Install);
        assert!(machine.begin_check(true));
        machine.finish_check(Ok(Some(found("0.2.0"))), NOW);
        machine
    }

    fn state(machine: &Machine) -> UpdateState {
        machine.status().state
    }

    #[test]
    fn it_starts_idle_with_nothing_found() {
        let machine = Machine::new(UpdateMode::Notify, Some("Because.".into()), "0.1.0");
        let status = machine.status();
        assert_eq!(status.mode, UpdateMode::Notify);
        assert_eq!(status.mode_reason.as_deref(), Some("Because."));
        assert_eq!(status.current_version, "0.1.0");
        assert_eq!(status.state, UpdateState::Idle);
        assert_eq!(status.available, None);
        assert_eq!(status.progress, None);
        assert_eq!(status.error, None);
        assert_eq!(status.error_kind, None);
        assert_eq!(status.last_checked_at, None);
        assert!(!status.manual);
    }

    #[test]
    fn a_check_starts_from_every_resting_state() {
        let mut machine = machine(UpdateMode::Install);
        assert!(machine.begin_check(false));
        assert_eq!(state(&machine), UpdateState::Checking);
        machine.finish_check(Ok(None), NOW);
        assert!(machine.begin_check(false), "after up to date");
        machine.finish_check(Err("offline".into()), NOW);
        assert!(machine.begin_check(false), "after an error");
        machine.finish_check(Ok(Some(found("0.2.0"))), NOW);
        assert!(machine.begin_check(false), "after an update was found");
    }

    #[test]
    fn a_check_while_one_runs_changes_nothing() {
        let mut machine = machine(UpdateMode::Install);
        assert!(machine.begin_check(false));
        let before = machine.status().clone();
        assert!(!machine.begin_check(true));
        assert_eq!(machine.status(), &before, "not even the manual flag");
    }

    #[test]
    fn a_check_during_a_download_or_install_changes_nothing() {
        let mut machine = available();
        machine.begin_download().unwrap();
        let downloading = machine.status().clone();
        assert!(!machine.begin_check(true));
        assert_eq!(machine.status(), &downloading);
        machine.begin_install();
        let installing = machine.status().clone();
        assert!(!machine.begin_check(true));
        assert_eq!(machine.status(), &installing);
    }

    #[test]
    fn an_install_that_never_looks_never_checks() {
        for mode in [UpdateMode::Off, UpdateMode::External] {
            let mut machine = machine(mode);
            assert!(!machine.begin_check(true), "{mode:?}");
            assert_eq!(state(&machine), UpdateState::Idle);
        }
    }

    #[test]
    fn an_install_that_only_announces_checks() {
        let mut machine = machine(UpdateMode::Notify);
        assert!(machine.begin_check(false));
    }

    #[test]
    fn the_manual_flag_follows_the_latest_check() {
        let mut machine = machine(UpdateMode::Install);
        machine.begin_check(true);
        assert!(machine.status().manual);
        machine.finish_check(Ok(None), NOW);
        assert!(machine.status().manual, "stays until the next check");
        machine.begin_check(false);
        assert!(!machine.status().manual);
    }

    #[test]
    fn nothing_new_is_up_to_date_and_remembers_when() {
        let mut machine = machine(UpdateMode::Install);
        machine.begin_check(false);
        machine.finish_check(Ok(None), NOW);
        let status = machine.status();
        assert_eq!(status.state, UpdateState::UpToDate);
        assert_eq!(status.last_checked_at, Some(NOW));
        assert_eq!(status.available, None);
        assert_eq!(status.error, None);
    }

    #[test]
    fn the_same_or_an_older_version_is_up_to_date() {
        for version in ["0.1.0", "0.0.9"] {
            let mut machine = machine(UpdateMode::Install);
            machine.begin_check(false);
            machine.finish_check(Ok(Some(found(version))), NOW);
            assert_eq!(state(&machine), UpdateState::UpToDate, "{version}");
            assert_eq!(machine.status().available, None);
        }
    }

    #[test]
    fn a_newer_version_is_available_with_clean_notes_and_a_link() {
        let machine = available();
        let status = machine.status();
        assert_eq!(status.state, UpdateState::Available);
        assert_eq!(
            status.available,
            Some(AvailableUpdate {
                version: "0.2.0".into(),
                date: Some("2026-10-22".into()),
                notes: "What's new".into(),
                release_url: "https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0".into(),
            })
        );
        assert_eq!(status.error, None);
        assert_eq!(status.last_checked_at, Some(NOW));
    }

    #[test]
    fn an_update_without_notes_or_a_date_has_empty_notes_and_no_date() {
        let mut machine = machine(UpdateMode::Install);
        machine.begin_check(false);
        let bare = Found {
            version: "0.2.0".into(),
            date: None,
            notes: None,
        };
        machine.finish_check(Ok(Some(bare)), NOW);
        let update = machine.status().available.clone().unwrap();
        assert_eq!(update.notes, "");
        assert_eq!(update.date, None);
    }

    #[test]
    fn a_version_that_is_not_plain_is_unreadable_and_has_no_link() {
        for version in ["0.2", "0.2.0-rc.1", "../x", "", "v0.2.0"] {
            let mut machine = machine(UpdateMode::Install);
            machine.begin_check(false);
            machine.finish_check(Ok(Some(found(version))), NOW);
            let status = machine.status();
            assert_eq!(status.state, UpdateState::Error, "{version:?}");
            assert_eq!(status.error.as_deref(), Some(UNREADABLE));
            assert_eq!(status.error_kind, Some(UpdateErrorKind::Check));
            assert_eq!(status.available, None);
        }
    }

    #[test]
    fn a_failed_check_is_an_error_and_clears_what_was_available() {
        let mut machine = available();
        assert!(machine.begin_check(false));
        assert!(machine.status().available.is_some(), "kept while the check runs");
        machine.finish_check(Err("The update server can't be reached.".into()), NOW + 5);
        let status = machine.status();
        assert_eq!(status.state, UpdateState::Error);
        assert_eq!(status.error.as_deref(), Some("The update server can't be reached."));
        assert_eq!(status.error_kind, Some(UpdateErrorKind::Check));
        assert_eq!(status.available, None);
        assert_eq!(status.last_checked_at, Some(NOW + 5));
    }

    #[test]
    fn the_next_check_clears_the_error() {
        let mut machine = machine(UpdateMode::Install);
        machine.begin_check(false);
        machine.finish_check(Err("offline".into()), NOW);
        machine.begin_check(false);
        assert_eq!(machine.status().error, None);
        assert_eq!(machine.status().error_kind, None);
        assert_eq!(state(&machine), UpdateState::Checking);
    }

    #[test]
    fn only_a_state_of_error_names_a_failed_step() {
        let mut machine = available();
        assert_eq!(machine.status().error_kind, None, "available");
        machine.begin_download().unwrap();
        assert_eq!(machine.status().error_kind, None, "downloading");
        machine.begin_install();
        assert_eq!(machine.status().error_kind, None, "installing");
        machine.fail_install("Couldn't install the update.".into());
        assert_eq!(machine.status().error_kind, Some(UpdateErrorKind::Install));
        machine.begin_check(true);
        machine.finish_check(Ok(None), NOW);
        assert_eq!(machine.status().error_kind, None, "up to date");
    }

    #[test]
    fn finishing_a_check_that_was_not_started_does_nothing() {
        let mut machine = machine(UpdateMode::Install);
        let before = machine.status().clone();
        machine.finish_check(Ok(Some(found("0.2.0"))), NOW);
        assert_eq!(machine.status(), &before);
    }

    #[test]
    fn an_update_downloads_then_installs() {
        let mut machine = available();
        machine.begin_download().unwrap();
        assert_eq!(state(&machine), UpdateState::Downloading);
        assert_eq!(machine.status().progress, None, "the size isn't known yet");
        assert!(machine.status().available.is_some());
        machine.set_progress(Some(0.5));
        assert_eq!(machine.status().progress, Some(0.5));
        machine.begin_install();
        assert_eq!(state(&machine), UpdateState::Installing);
        assert_eq!(machine.status().progress, None);
        assert!(machine.status().available.is_some());
    }

    #[test]
    fn nothing_downloads_without_an_update() {
        let mut machine = machine(UpdateMode::Install);
        let error = machine.begin_download().unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Invalid);
        assert_eq!(state(&machine), UpdateState::Idle);
        machine.begin_check(false);
        machine.finish_check(Ok(None), NOW);
        assert_eq!(machine.begin_download().unwrap_err().kind, CommandErrorKind::Invalid);
    }

    #[test]
    fn an_install_that_only_announces_never_downloads() {
        let mut machine = machine(UpdateMode::Notify);
        machine.begin_check(false);
        machine.finish_check(Ok(Some(found("0.2.0"))), NOW);
        assert_eq!(state(&machine), UpdateState::Available);
        let error = machine.begin_download().unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Invalid);
        assert_eq!(state(&machine), UpdateState::Available, "still available to read about");
    }

    #[test]
    fn a_second_click_while_downloading_is_refused() {
        let mut machine = available();
        machine.begin_download().unwrap();
        assert_eq!(machine.begin_download().unwrap_err().kind, CommandErrorKind::Invalid);
        assert_eq!(state(&machine), UpdateState::Downloading);
    }

    #[test]
    fn progress_is_clamped_and_only_a_download_has_one() {
        let mut machine = available();
        machine.set_progress(Some(0.4));
        assert_eq!(machine.status().progress, None, "not downloading");
        machine.begin_download().unwrap();
        machine.set_progress(Some(1.7));
        assert_eq!(machine.status().progress, Some(1.0));
        machine.set_progress(Some(-3.0));
        assert_eq!(machine.status().progress, Some(0.0));
        machine.set_progress(Some(f64::NAN));
        assert_eq!(machine.status().progress, None);
        machine.set_progress(None);
        assert_eq!(machine.status().progress, None);
    }

    #[test]
    fn a_failed_download_is_an_error_that_still_names_the_update() {
        let mut machine = available();
        machine.begin_download().unwrap();
        machine.set_progress(Some(0.3));
        machine.fail_install("Couldn't install the update.".into());
        let status = machine.status();
        assert_eq!(status.state, UpdateState::Error);
        assert_eq!(status.error.as_deref(), Some("Couldn't install the update."));
        assert_eq!(status.error_kind, Some(UpdateErrorKind::Install));
        assert_eq!(status.progress, None);
        assert_eq!(status.available.as_ref().map(|u| u.version.as_str()), Some("0.2.0"));
    }

    #[test]
    fn after_a_failed_install_a_check_and_a_new_try_are_possible() {
        let mut machine = available();
        machine.begin_download().unwrap();
        machine.begin_install();
        machine.fail_install("Couldn't install the update.".into());
        assert!(machine.begin_check(true));
        machine.finish_check(Ok(Some(found("0.2.0"))), NOW);
        machine.begin_download().unwrap();
        assert_eq!(state(&machine), UpdateState::Downloading);
        assert_eq!(machine.status().error, None);
    }
}
