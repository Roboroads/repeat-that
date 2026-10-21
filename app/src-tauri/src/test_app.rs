//! A mock app for tests of the commands: every command, fake settings storage, autostart and shortcuts, and optionally
//! the real audio engine capturing the rt-core speech fixture as a fake app, playing into memory. Nothing is written to
//! disk; the only file read is the fixture.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rt_capture::MockSource;
use rt_engine::TestOutput;
use serde_json::Value;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;
use tauri::{Listener, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::actions::SharedEngine;
use crate::autostart::fake::FakeAutostart;
use crate::commands;
use crate::engine::{self, AppEngine, STATUS_EVENT, SharedAppEngine};
use crate::hotkeys::fake::FakeBackend;
use crate::icons::{IconCache, SharedIcons};
use crate::service::fake::MemoryPersistence;
use crate::service::{SETTINGS_CHANGED_EVENT, install_with};
use crate::tray::SharedStatus;

/// The speech fixture, which plays as the fake app "speech_8s".
pub const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/rt-core/tests/fixtures/speech_8s.wav"
);

pub struct TestApp {
    pub app: tauri::App<MockRuntime>,
    pub window: WebviewWindow<MockRuntime>,
    pub persistence: Arc<MemoryPersistence>,
    pub backend: Arc<FakeBackend>,
    /// What replays played, when the engine runs.
    pub output: TestOutput,
    events: Arc<Mutex<Vec<(String, Value)>>>,
}

impl TestApp {
    /// The settings commands work; the engine doesn't run.
    pub fn without_engine() -> Self {
        Self::build(false, None)
    }

    /// The engine runs on the fixture, played in real time and over and over like a call.
    pub fn with_engine() -> Self {
        Self::build(true, None)
    }

    /// [`with_engine`](Self::with_engine), launched with `stored` in the settings file.
    pub fn with_engine_and_settings(stored: Value) -> Self {
        Self::build(true, Some(stored))
    }

    fn build(with_engine: bool, stored: Option<Value>) -> Self {
        let app = mock_builder()
            .invoke_handler(commands::handler())
            .manage::<SharedStatus>(Mutex::default())
            .manage::<SharedIcons>(Arc::new(IconCache::default()))
            .build(mock_context(noop_assets()))
            .expect("mock app builds");
        let engine: SharedAppEngine = Arc::new(AppEngine::default());
        app.manage::<SharedEngine>(engine.clone());
        app.manage::<SharedAppEngine>(engine);

        let events = Arc::new(Mutex::new(Vec::new()));
        for name in [SETTINGS_CHANGED_EVENT, STATUS_EVENT] {
            let sink = Arc::clone(&events);
            app.listen(name, move |event| {
                let payload = serde_json::from_str(event.payload()).expect("the event carries JSON");
                sink.lock().unwrap().push((name.to_string(), payload));
            });
        }

        let persistence = Arc::new(stored.map_or_else(MemoryPersistence::default, MemoryPersistence::holding));
        let backend = Arc::new(FakeBackend::default());
        install_with(
            app.handle(),
            persistence.clone(),
            Arc::new(FakeAutostart::default()),
            Box::new(Arc::clone(&backend)),
        );
        engine::start_publishing(app.handle());
        let output = TestOutput::new(48_000);
        if with_engine {
            let source = MockSource::from_wav(FIXTURE).expect("the fixture plays");
            engine::install_with(app.handle(), Arc::new(source), Box::new(output.clone()));
        }
        let window = WebviewWindowBuilder::new(&app, "settings", WebviewUrl::default())
            .build()
            .expect("mock window builds");
        TestApp {
            app,
            window,
            persistence,
            backend,
            output,
            events,
        }
    }

    /// Invokes `cmd` the way the frontend does and returns what the promise would resolve or reject with.
    pub fn invoke(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        // The origin of the app's own pages, which is what lets a command through without a capability.
        let origin = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        get_ipc_response(
            &self.window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: origin.parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize().expect("the response is JSON"))
    }

    /// The payloads of the `name` events so far.
    pub fn events(&self, name: &str) -> Vec<Value> {
        let events = self.events.lock().unwrap();
        events
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, p)| p.clone())
            .collect()
    }

    pub fn clear_events(&self) {
        self.events.lock().unwrap().clear();
    }

    /// Asks `get_status` until `done` holds, for up to `timeout`, and returns that status.
    pub fn wait_for_status(&self, timeout: Duration, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            let status = self.invoke("get_status", Value::Null).expect("get_status answers");
            if done(&status) {
                return status;
            }
            assert!(
                start.elapsed() < timeout,
                "{what} within {timeout:?}; last status {status}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until `done` holds, for up to `timeout`.
    pub fn wait_until(&self, timeout: Duration, what: &str, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < timeout, "{what} within {timeout:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for a `status` event for which `done` holds.
    pub fn wait_for_event(&self, timeout: Duration, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            if let Some(status) = self.events(STATUS_EVENT).into_iter().find(&done) {
                return status;
            }
            assert!(
                start.elapsed() < timeout,
                "a status event with {what} within {timeout:?}; got {:?}",
                self.events(STATUS_EVENT)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for TestApp {
    /// The status threads keep the app alive, so the engine and the publisher are stopped here, as when the app ends.
    fn drop(&mut self) {
        engine::shutdown(self.app.handle());
    }
}
