//! Where updates come from: [`PluginSource`] asks GitHub through the updater plugin, and [`MockSource`] stands in for it
//! in debug builds so the window and the end-to-end tests can see every state without a network.
//!
//! Both hand back plain strings for failures. They are sentences for the user: the details go to stderr, where the
//! person who has to look into it can find them.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::{Error, Update, UpdaterExt};

use crate::locking::lock;

/// An update the server announced, before the app has checked it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub version: String,
    /// The day it was released, `YYYY-MM-DD`.
    pub date: Option<String>,
    pub notes: Option<String>,
}

/// What the update service needs from the outside world. Every method blocks, so they are called from threads that may.
pub trait UpdateSource: Send + Sync {
    /// Asks whether there is a newer version. `Ok(None)` = no, or none for this system yet.
    fn check(&self) -> Result<Option<Found>, String>;
    /// Downloads the update the last check found and verifies its signature. `progress` hears how many bytes have
    /// arrived and how many there will be, when that is known.
    fn download(&self, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String>;
    /// Installs the downloaded update. On Windows the installer takes over and the process ends here.
    fn install(&self) -> Result<(), String>;
}

// What the user is told

pub const CHECK_UNREACHABLE: &str = "Couldn't check for updates: the update server can't be reached.";
pub const CHECK_FAILED: &str = "Couldn't check for updates.";
pub const CHECK_UNREADABLE: &str = "Couldn't read the update information.";
pub const DOWNLOAD_FAILED: &str = "Couldn't install the update: the download failed.";
pub const BAD_SIGNATURE: &str = "Couldn't install the update: it didn't pass the signature check.";
pub const WRONG_FORMAT: &str = "Couldn't install the update: it isn't a file this version can install.";
pub const INSTALL_FAILED: &str = "Couldn't install the update.";
const NOTHING_TO_INSTALL: &str = "There is no downloaded update to install.";

/// What a check that failed in the plugin means: `Ok(None)` when there is simply no update for this system, else the
/// sentence for the user.
pub fn check_outcome(error: &Error) -> Result<Option<Found>, String> {
    match error {
        // A 404 (the file for this system isn't published yet, or the release was made without the update key) comes
        // out of the plugin as "not found", and so does any other answer that isn't a manifest.
        Error::ReleaseNotFound | Error::TargetNotFound(_) | Error::TargetsNotFound(_) => Ok(None),
        Error::Serialization(_) | Error::Semver(_) | Error::UrlParse(_) => Err(CHECK_UNREADABLE.to_string()),
        Error::Reqwest(error) if error.is_decode() => Err(CHECK_UNREADABLE.to_string()),
        Error::Reqwest(_) | Error::Network(_) | Error::Io(_) => Err(CHECK_UNREACHABLE.to_string()),
        _ => Err(CHECK_FAILED.to_string()),
    }
}

/// The sentence for a download that failed.
pub fn download_error(error: &Error) -> String {
    match error {
        Error::Reqwest(_) | Error::Network(_) | Error::Io(_) => DOWNLOAD_FAILED,
        error if is_signature_problem(error) => BAD_SIGNATURE,
        _ => INSTALL_FAILED,
    }
    .to_string()
}

/// The sentence for an install that failed.
pub fn install_error(error: &Error) -> String {
    match error {
        error if is_signature_problem(error) => BAD_SIGNATURE,
        Error::InvalidUpdaterFormat | Error::BinaryNotFoundInArchive => WRONG_FORMAT,
        _ => INSTALL_FAILED,
    }
    .to_string()
}

/// The signature, or the version it was made for, didn't match.
fn is_signature_problem(error: &Error) -> bool {
    matches!(
        error,
        Error::Minisign(_)
            | Error::Base64(_)
            | Error::SignatureUtf8(_)
            | Error::SignedVersionMismatch { .. }
            | Error::MissingSignedVersion
    )
}

// The real source

/// How long a request waits to connect and for the next piece of an answer. A stalled connection would otherwise leave
/// the window on "Checking…" forever. A download as a whole has no limit: it may be big and the link slow.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The updater plugin, asking the endpoint it was set up with and checking every signature against the public key it
/// was given. Holds on to the update the last check found, and to its bytes once downloaded.
pub struct PluginSource<R: Runtime> {
    app: AppHandle<R>,
    found: Mutex<Option<Update>>,
    downloaded: Mutex<Option<Vec<u8>>>,
}

impl<R: Runtime> PluginSource<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        PluginSource {
            app,
            found: Mutex::new(None),
            downloaded: Mutex::new(None),
        }
    }
}

impl<R: Runtime> UpdateSource for PluginSource<R> {
    fn check(&self) -> Result<Option<Found>, String> {
        *lock(&self.found) = None;
        *lock(&self.downloaded) = None;
        let updater = self
            .app
            .updater_builder()
            .configure_client(|client| client.connect_timeout(CONNECT_TIMEOUT).read_timeout(READ_TIMEOUT))
            .build()
            .map_err(|error| {
                eprintln!("could not set up the update check: {error}");
                CHECK_FAILED.to_string()
            })?;
        match tauri::async_runtime::block_on(updater.check()) {
            Ok(Some(update)) => {
                let found = Found {
                    version: update.version.clone(),
                    date: update.date.map(|date| date.date().to_string()),
                    notes: update.body.clone(),
                };
                *lock(&self.found) = Some(update);
                Ok(Some(found))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                eprintln!("update check failed: {error}");
                check_outcome(&error)
            }
        }
    }

    fn download(&self, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String> {
        let update = lock(&self.found).clone().ok_or(NOTHING_TO_INSTALL)?;
        let mut received = 0u64;
        let bytes = tauri::async_runtime::block_on(update.download(
            |chunk, total| {
                received += chunk as u64;
                progress(received, total);
            },
            || {},
        ))
        .map_err(|error| {
            eprintln!("update download failed: {error}");
            download_error(&error)
        })?;
        *lock(&self.downloaded) = Some(bytes);
        Ok(())
    }

    fn install(&self) -> Result<(), String> {
        let update = lock(&self.found).clone().ok_or(NOTHING_TO_INSTALL)?;
        let bytes = lock(&self.downloaded).take().ok_or(NOTHING_TO_INSTALL)?;
        update.install(bytes).map_err(|error| {
            eprintln!("update install failed: {error}");
            install_error(&error)
        })
    }
}

// The mock

/// The environment variable that picks the mock, in debug builds only.
pub const MOCK_VAR: &str = "REPEAT_THAT_MOCK_UPDATE";

/// What the mock plays out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockKind {
    /// `99.0.0` is available and downloads and installs.
    Available,
    /// Nothing newer.
    None,
    /// The check fails.
    FailCheck,
    /// `99.0.0` is available, but its download fails.
    FailDownload,
}

/// The mock to use: only a debug build has one, and only for a value that names one.
pub fn mock_kind(debug_build: bool, value: Option<&str>) -> Option<MockKind> {
    if !debug_build {
        return None;
    }
    match value? {
        "available" => Some(MockKind::Available),
        "none" => Some(MockKind::None),
        "fail-check" => Some(MockKind::FailCheck),
        "fail-download" => Some(MockKind::FailDownload),
        _ => None,
    }
}

/// How long the mock takes over each step, so the window has time to show them.
#[derive(Clone, Copy, Debug)]
pub struct MockPace {
    pub check: Duration,
    pub download_step: Duration,
    pub install: Duration,
}

impl MockPace {
    /// A download of about two seconds.
    pub const SLOW: MockPace = MockPace {
        check: Duration::from_millis(400),
        download_step: Duration::from_millis(100),
        install: Duration::from_millis(300),
    };
}

pub struct MockSource {
    kind: MockKind,
    pace: MockPace,
}

/// The version the mock announces.
pub const MOCK_VERSION: &str = "99.0.0";
const MOCK_NOTES: &str = "- A sample of release notes.\n- Nothing here is real: this update comes from the mock source \
                          (REPEAT_THAT_MOCK_UPDATE), and nothing is installed.";
/// The mock's download takes this many steps.
const MOCK_STEPS: u64 = 20;
/// The step at which `fail-download` fails.
const MOCK_FAIL_AT: u64 = 8;

impl MockSource {
    pub fn new(kind: MockKind, pace: MockPace) -> Self {
        MockSource { kind, pace }
    }
}

impl UpdateSource for MockSource {
    fn check(&self) -> Result<Option<Found>, String> {
        std::thread::sleep(self.pace.check);
        match self.kind {
            MockKind::Available | MockKind::FailDownload => Ok(Some(Found {
                version: MOCK_VERSION.to_string(),
                date: Some("2026-01-15".to_string()),
                notes: Some(MOCK_NOTES.to_string()),
            })),
            MockKind::None => Ok(None),
            MockKind::FailCheck => Err(CHECK_UNREACHABLE.to_string()),
        }
    }

    fn download(&self, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String> {
        let steps = if self.kind == MockKind::FailDownload {
            MOCK_FAIL_AT
        } else {
            MOCK_STEPS
        };
        for step in 1..=steps {
            std::thread::sleep(self.pace.download_step);
            progress(step * 100 / MOCK_STEPS, Some(100));
        }
        if self.kind == MockKind::FailDownload {
            return Err(DOWNLOAD_FAILED.to_string());
        }
        Ok(())
    }

    fn install(&self) -> Result<(), String> {
        std::thread::sleep(self.pace.install);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTANT: MockPace = MockPace {
        check: Duration::ZERO,
        download_step: Duration::ZERO,
        install: Duration::ZERO,
    };

    #[test]
    fn a_mock_is_picked_by_name() {
        for (value, kind) in [
            ("available", MockKind::Available),
            ("none", MockKind::None),
            ("fail-check", MockKind::FailCheck),
            ("fail-download", MockKind::FailDownload),
        ] {
            assert_eq!(mock_kind(true, Some(value)), Some(kind), "{value}");
        }
    }

    #[test]
    fn a_release_build_has_no_mock() {
        for value in ["available", "none", "fail-check", "fail-download"] {
            assert_eq!(mock_kind(false, Some(value)), None, "{value}");
        }
    }

    #[test]
    fn anything_else_is_no_mock() {
        for value in [
            None,
            Some(""),
            Some("1"),
            Some("Available"),
            Some("available "),
            Some("fail"),
        ] {
            assert_eq!(mock_kind(true, value), None, "{value:?}");
        }
    }

    #[test]
    fn the_available_mock_announces_a_big_version_with_notes() {
        let found = MockSource::new(MockKind::Available, INSTANT).check().unwrap().unwrap();
        assert_eq!(found.version, MOCK_VERSION);
        assert!(found.date.is_some());
        assert!(found.notes.is_some_and(|notes| notes.contains("sample")));
    }

    #[test]
    fn the_none_mock_has_nothing() {
        assert_eq!(MockSource::new(MockKind::None, INSTANT).check(), Ok(None));
    }

    #[test]
    fn the_failing_check_mock_fails_the_way_a_real_check_does() {
        assert_eq!(
            MockSource::new(MockKind::FailCheck, INSTANT).check(),
            Err(CHECK_UNREACHABLE.to_string())
        );
    }

    #[test]
    fn the_failing_download_mock_still_finds_the_update() {
        let found = MockSource::new(MockKind::FailDownload, INSTANT).check().unwrap();
        assert_eq!(found.map(|f| f.version), Some(MOCK_VERSION.to_string()));
    }

    /// What the download reported: how many bytes had arrived and how many there would be.
    type Reports = Vec<(u64, Option<u64>)>;

    fn download(kind: MockKind) -> (Result<(), String>, Reports) {
        let mut seen = Vec::new();
        let result = MockSource::new(kind, INSTANT).download(&mut |received, total| seen.push((received, total)));
        (result, seen)
    }

    #[test]
    fn the_mock_download_fakes_progress_up_to_the_whole() {
        let (result, seen) = download(MockKind::Available);
        assert_eq!(result, Ok(()));
        assert_eq!(seen.len() as u64, MOCK_STEPS);
        assert_eq!(seen.last(), Some(&(100, Some(100))));
        assert!(seen.windows(2).all(|pair| pair[0].0 < pair[1].0), "{seen:?}");
        assert!(seen.iter().all(|(_, total)| *total == Some(100)));
    }

    #[test]
    fn the_failing_download_mock_stops_part_of_the_way() {
        let (result, seen) = download(MockKind::FailDownload);
        assert_eq!(result, Err(DOWNLOAD_FAILED.to_string()));
        assert_eq!(seen.len() as u64, MOCK_FAIL_AT);
        assert!(seen.last().unwrap().0 < 100);
    }

    #[test]
    fn the_mock_installs_and_leaves_it_at_that() {
        assert_eq!(MockSource::new(MockKind::Available, INSTANT).install(), Ok(()));
    }

    // What the plugin's errors mean

    #[test]
    fn no_release_file_or_none_for_this_system_is_no_update() {
        // GitHub answers 404 for a manifest that isn't there (yet), which the plugin reports as not found.
        assert_eq!(check_outcome(&Error::ReleaseNotFound), Ok(None));
        assert_eq!(check_outcome(&Error::TargetNotFound("linux-x86_64".into())), Ok(None));
        assert_eq!(
            check_outcome(&Error::TargetsNotFound(vec!["linux-x86_64".into()])),
            Ok(None)
        );
    }

    #[test]
    fn a_check_that_could_not_get_an_answer_says_the_server_cannot_be_reached() {
        let error = Error::Network("connection reset".into());
        assert_eq!(check_outcome(&error), Err(CHECK_UNREACHABLE.to_string()));
        let error = Error::Io(std::io::Error::other("no route"));
        assert_eq!(check_outcome(&error), Err(CHECK_UNREACHABLE.to_string()));
    }

    #[test]
    fn a_manifest_that_cannot_be_read_says_so() {
        let bad_json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        assert_eq!(
            check_outcome(&Error::Serialization(bad_json)),
            Err(CHECK_UNREADABLE.to_string())
        );
    }

    #[test]
    fn any_other_check_error_is_a_plain_failure() {
        assert_eq!(check_outcome(&Error::EmptyEndpoints), Err(CHECK_FAILED.to_string()));
        assert_eq!(check_outcome(&Error::UnsupportedArch), Err(CHECK_FAILED.to_string()));
    }

    #[test]
    fn a_signature_that_does_not_fit_is_named_as_such() {
        let signed_for_another = Error::SignedVersionMismatch {
            signed: "0.1.0".into(),
            announced: "0.9.0".into(),
        };
        for error in [signed_for_another, Error::MissingSignedVersion] {
            assert_eq!(download_error(&error), BAD_SIGNATURE, "{error}");
            assert_eq!(install_error(&error), BAD_SIGNATURE, "{error}");
        }
    }

    #[test]
    fn a_download_that_broke_off_says_so() {
        let error = Error::Network("Download request failed with status: 404".into());
        assert_eq!(download_error(&error), DOWNLOAD_FAILED);
        assert_eq!(download_error(&Error::Io(std::io::Error::other("x"))), DOWNLOAD_FAILED);
    }

    #[test]
    fn a_file_that_is_not_an_update_for_this_system_says_so() {
        assert_eq!(install_error(&Error::InvalidUpdaterFormat), WRONG_FORMAT);
        assert_eq!(install_error(&Error::BinaryNotFoundInArchive), WRONG_FORMAT);
    }

    #[test]
    fn any_other_install_error_is_a_plain_failure() {
        assert_eq!(install_error(&Error::TempDirNotOnSameMountPoint), INSTALL_FAILED);
        assert_eq!(install_error(&Error::AuthenticationFailed), INSTALL_FAILED);
        assert_eq!(download_error(&Error::EmptyEndpoints), INSTALL_FAILED);
    }

    /// The plugin itself, asking a server on this machine.
    mod plugin {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        use base64::Engine;
        use base64::engine::general_purpose::STANDARD as BASE64;
        use serde_json::json;
        use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};

        use super::*;

        /// The public key and a signature of the minisign-verify crate's own tests (its documentation names them): the
        /// key can only check, nothing is signed with it here. `SIGNATURE` is of the bytes `test`.
        const PUBLIC_KEY: &str = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n";
        const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
            RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\n\
            trusted comment: timestamp:1555779966\tfile:test\n\
            QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";

        /// A server on this machine that answers each path of `routes` with its status and body, and everything else
        /// with 404. The routes are made from the port, which the manifest of a release has to name. Returns the port.
        fn serve(routes: impl FnOnce(u16) -> Vec<(String, u16, Vec<u8>)>) -> u16 {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
            let port = listener.local_addr().unwrap().port();
            let routes = routes(port);
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut reader = BufReader::new(stream);
                    let mut request = String::new();
                    if reader.read_line(&mut request).is_err() {
                        continue;
                    }
                    // The rest of the request, up to the empty line.
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                        line.clear();
                    }
                    let path = request.split_whitespace().nth(1).unwrap_or_default();
                    let (status, body) = routes
                        .iter()
                        .find(|(route, _, _)| route == path)
                        .map_or((404, Vec::new()), |(_, status, body)| (*status, body.clone()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let stream = reader.get_mut();
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
            port
        }

        fn manifest_path() -> String {
            format!(
                "/latest-{}.json",
                tauri_plugin_updater::target().expect("a supported system")
            )
        }

        fn manifest(version: &str, signature: &str, port: u16) -> Vec<u8> {
            let target = tauri_plugin_updater::target().expect("a supported system");
            json!({
                "version": version,
                "notes": "Notes <!-- marker -->",
                "pub_date": "2026-10-22T12:00:00Z",
                "platforms": { target: { "url": format!("http://127.0.0.1:{port}/payload"), "signature": BASE64.encode(signature) } }
            })
            .to_string()
            .into_bytes()
        }

        fn source(port: u16) -> (tauri::App<MockRuntime>, PluginSource<MockRuntime>) {
            let mut context = mock_context(noop_assets());
            // The mock app would otherwise report a version of its own, not the one this crate is released as.
            context.package_info_mut().version = env!("CARGO_PKG_VERSION").parse().unwrap();
            let endpoint = format!("http://127.0.0.1:{port}/latest-{{{{target}}}}-{{{{arch}}}}.json");
            context.config_mut().plugins.0.insert(
                "updater".into(),
                crate::updater::plugin_config(&BASE64.encode(PUBLIC_KEY), Some(&endpoint)),
            );
            let app = mock_builder()
                .plugin(tauri_plugin_updater::Builder::new().build())
                .build(context)
                .expect("the plugin accepts the configuration the app makes for it");
            let source = PluginSource::new(app.handle().clone());
            (app, source)
        }

        /// A server with the manifest of an update to `version` and `payload` to download.
        fn with_release(
            version: &str,
            signature: &str,
            payload: &[u8],
        ) -> (tauri::App<MockRuntime>, PluginSource<MockRuntime>) {
            source(serve(|port| {
                vec![
                    (manifest_path(), 200, manifest(version, signature, port)),
                    ("/payload".to_string(), 200, payload.to_vec()),
                ]
            }))
        }

        #[test]
        fn a_newer_release_is_found_with_its_notes_and_day() {
            let (_app, source) = with_release("99.0.0", SIGNATURE, b"test");
            let found = source.check().unwrap().expect("an update");
            assert_eq!(found.version, "99.0.0");
            assert_eq!(found.date.as_deref(), Some("2026-10-22"));
            assert_eq!(found.notes.as_deref(), Some("Notes <!-- marker -->"));
        }

        #[test]
        fn the_running_version_is_no_update() {
            let (_app, source) = with_release(env!("CARGO_PKG_VERSION"), SIGNATURE, b"test");
            assert_eq!(source.check(), Ok(None));
        }

        #[test]
        fn a_manifest_that_is_not_there_is_no_update() {
            // GitHub's 404 for a release file that isn't attached (yet).
            let port = serve(|_| Vec::new());
            let (_app, source) = source(port);
            assert_eq!(source.check(), Ok(None));
        }

        #[test]
        fn a_manifest_that_is_not_one_cannot_be_read() {
            for body in ["{}", "not json at all"] {
                let port = serve(|_| vec![(manifest_path(), 200, body.as_bytes().to_vec())]);
                let (_app, source) = source(port);
                assert_eq!(source.check(), Err(CHECK_UNREADABLE.to_string()), "{body}");
            }
        }

        #[test]
        fn a_release_without_a_build_for_this_system_is_no_update() {
            let body = json!({
                "version": "99.0.0",
                "platforms": { "plan9-mips": { "url": "http://127.0.0.1:1/x", "signature": "x" } }
            });
            let port = serve(|_| vec![(manifest_path(), 200, body.to_string().into_bytes())]);
            let (_app, source) = source(port);
            assert_eq!(source.check(), Ok(None));
        }

        #[test]
        fn a_server_that_cannot_be_reached_says_so() {
            let free = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = free.local_addr().unwrap().port();
            drop(free);
            let (_app, source) = source(port);
            assert_eq!(source.check(), Err(CHECK_UNREACHABLE.to_string()));
        }

        #[test]
        fn a_download_signed_by_the_key_is_kept_and_reports_its_size() {
            let (_app, source) = with_release("99.0.0", SIGNATURE, b"test");
            source.check().unwrap().expect("an update");
            let mut seen = Vec::new();
            assert_eq!(
                source.download(&mut |received, total| seen.push((received, total))),
                Ok(())
            );
            assert_eq!(seen.last(), Some(&(4, Some(4))), "{seen:?}");
            assert!(lock(&source.downloaded).is_some(), "ready to install");
        }

        #[test]
        fn a_download_the_key_did_not_sign_is_refused_and_nothing_is_kept() {
            // The signature is for `test`; the server has `Test`.
            let (_app, source) = with_release("99.0.0", SIGNATURE, b"Test");
            source.check().unwrap().expect("an update");
            assert_eq!(source.download(&mut |_, _| {}), Err(BAD_SIGNATURE.to_string()));
            assert!(lock(&source.downloaded).is_none());
            // So there is nothing to install, and the installer is not even asked.
            assert_eq!(source.install(), Err(NOTHING_TO_INSTALL.to_string()));
        }

        #[test]
        fn a_signature_that_is_rubbish_is_refused_too() {
            let (_app, source) = with_release("99.0.0", "not a signature", b"test");
            source.check().unwrap().expect("an update");
            assert_eq!(source.download(&mut |_, _| {}), Err(BAD_SIGNATURE.to_string()));
        }

        #[test]
        fn a_download_that_is_not_there_says_the_download_failed() {
            // The manifest names a payload the server doesn't have.
            let port = serve(|port| vec![(manifest_path(), 200, manifest("99.0.0", SIGNATURE, port))]);
            let (_app, source) = source(port);
            source.check().unwrap().expect("an update");
            assert_eq!(source.download(&mut |_, _| {}), Err(DOWNLOAD_FAILED.to_string()));
        }

        #[test]
        fn nothing_downloads_before_a_check_found_something() {
            let port = serve(|_| Vec::new());
            let (_app, source) = source(port);
            assert_eq!(source.download(&mut |_, _| {}), Err(NOTHING_TO_INSTALL.to_string()));
        }
    }

    #[test]
    fn every_failure_sentence_is_one_the_window_can_show_as_is() {
        for sentence in [
            CHECK_UNREACHABLE,
            CHECK_FAILED,
            CHECK_UNREADABLE,
            DOWNLOAD_FAILED,
            BAD_SIGNATURE,
            WRONG_FORMAT,
            INSTALL_FAILED,
        ] {
            assert!(sentence.ends_with('.') && !sentence.contains('\n'), "{sentence}");
        }
    }
}
