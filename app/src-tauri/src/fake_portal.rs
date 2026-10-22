//! A stand-in for the desktop's GlobalShortcuts portal, on a bus of its own, for the tests of the code that talks to the
//! real one. It does what the interface description says and nothing more: a session per `CreateSession`, answers to
//! `BindShortcuts` through `Request` objects, and signals the test sends when it wants a key pressed, a key changed or the
//! session ended. No desktop is involved, so what it shows is that the app speaks the protocol, not that a particular
//! desktop answers the way the interface says.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Type, as_value};
use zbus::{Connection, fdo};

use crate::private_bus::PrivateBus;

const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";

/// What `BindShortcuts` is answered with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BindAnswer {
    /// Every shortcut gets its preferred key, or none when it had none (a dialog the user accepted).
    #[default]
    Accept,
    /// The user closed the dialog.
    Dismiss,
    /// The method call fails.
    Fail,
    /// The dialog stays open until the test calls [`FakePortal::release_held`], which accepts it.
    Hold,
}

/// One shortcut of a `BindShortcuts` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub id: String,
    pub description: String,
    pub preferred_trigger: Option<String>,
}

#[derive(Default)]
pub struct State {
    /// The `BindShortcuts` calls so far, each with the shortcuts it asked for.
    pub binds: Mutex<Vec<Vec<Asked>>>,
    pub configures: AtomicUsize,
    pub answer: Mutex<BindAnswer>,
    session: Mutex<Option<String>>,
    /// The answer of a held bind, waiting to be sent.
    held: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

pub struct FakePortal {
    connection: Connection,
    pub state: Arc<State>,
}

#[derive(Serialize, Type)]
#[zvariant(signature = "dict")]
struct SessionCreated {
    #[serde(with = "as_value")]
    session_handle: String,
}

#[derive(Serialize, Type)]
#[zvariant(signature = "dict")]
struct ShortcutInfo {
    #[serde(with = "as_value")]
    description: String,
    #[serde(with = "as_value")]
    trigger_description: String,
}

#[derive(Serialize, Type)]
struct ShortcutOut(String, ShortcutInfo);

#[derive(Serialize, Type)]
#[zvariant(signature = "dict")]
struct Bound {
    #[serde(with = "as_value")]
    shortcuts: Vec<ShortcutOut>,
}

fn shortcut_out(id: &str, trigger: &str) -> ShortcutOut {
    ShortcutOut(
        id.to_string(),
        ShortcutInfo {
            description: id.to_string(),
            trigger_description: trigger.to_string(),
        },
    )
}

struct Shortcuts {
    version: u32,
    state: Arc<State>,
}

/// The part of a unique name that appears in object paths: `:1.42` is `1_42`.
fn sender_in_path(header: &Header<'_>) -> String {
    header
        .sender()
        .map(|sender| sender.as_str().trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default()
}

fn token(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options
        .get(key)
        .and_then(|value| String::try_from(value.clone()).ok())
        .unwrap_or_else(|| panic!("the call has no {key}"))
}

/// Sends the `Response` of a request a moment from now, the way a portal answers after the user did something.
fn respond(connection: Connection, request: String, code: u32, results: impl Serialize + Type + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        zbus::block_on(connection.emit_signal(
            None::<&str>,
            request,
            "org.freedesktop.portal.Request",
            "Response",
            &(code, results),
        ))
        .expect("the response goes out");
    });
}

#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl Shortcuts {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.version
    }

    async fn create_session(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        options: HashMap<String, OwnedValue>,
    ) -> fdo::Result<OwnedObjectPath> {
        let sender = sender_in_path(&header);
        let request = format!(
            "/org/freedesktop/portal/desktop/request/{sender}/{}",
            token(&options, "handle_token")
        );
        let session = format!(
            "/org/freedesktop/portal/desktop/session/{sender}/{}",
            token(&options, "session_handle_token")
        );
        *self.state.session.lock().unwrap() = Some(session.clone());
        respond(
            connection.clone(),
            request.clone(),
            0,
            SessionCreated {
                session_handle: session,
            },
        );
        OwnedObjectPath::try_from(request).map_err(|err| fdo::Error::Failed(err.to_string()))
    }

    async fn bind_shortcuts(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _session_handle: ObjectPath<'_>,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
    ) -> fdo::Result<OwnedObjectPath> {
        let asked: Vec<Asked> = shortcuts
            .into_iter()
            .map(|(id, info)| Asked {
                description: info
                    .get("description")
                    .and_then(|value| String::try_from(value.clone()).ok())
                    .unwrap_or_default(),
                preferred_trigger: info
                    .get("preferred_trigger")
                    .and_then(|value| String::try_from(value.clone()).ok()),
                id,
            })
            .collect();
        self.state.binds.lock().unwrap().push(asked.clone());
        let answer = *self.state.answer.lock().unwrap();
        if answer == BindAnswer::Fail {
            return Err(fdo::Error::Failed("the compositor refused".into()));
        }
        let request = format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender_in_path(&header),
            token(&options, "handle_token")
        );
        let accepted = Bound {
            shortcuts: asked
                .iter()
                .map(|shortcut| shortcut_out(&shortcut.id, shortcut.preferred_trigger.as_deref().unwrap_or("")))
                .collect(),
        };
        match answer {
            BindAnswer::Accept => respond(connection.clone(), request.clone(), 0, accepted),
            BindAnswer::Hold => {
                let (connection, request) = (connection.clone(), request.clone());
                *self.state.held.lock().unwrap() = Some(Box::new(move || respond(connection, request, 0, accepted)));
            }
            // The response of a dialog that was closed: code 1 and nothing else.
            _ => respond(
                connection.clone(),
                request.clone(),
                1,
                HashMap::<String, OwnedValue>::new(),
            ),
        }
        OwnedObjectPath::try_from(request).map_err(|err| fdo::Error::Failed(err.to_string()))
    }

    async fn configure_shortcuts(
        &self,
        _session_handle: ObjectPath<'_>,
        _parent_window: String,
        _options: HashMap<String, OwnedValue>,
    ) {
        self.state.configures.fetch_add(1, Ordering::SeqCst);
    }
}

/// A portal that has an interface, but not the shortcuts one: what a desktop whose portal has no backend for shortcuts
/// looks like.
struct Settings;

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl Settings {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

impl FakePortal {
    /// A portal with the shortcuts interface in `version`, on `bus`.
    pub fn start(bus: &PrivateBus, version: u32) -> Self {
        let portal = Self::start_without_shortcuts(bus);
        portal.add_shortcuts(version);
        portal
    }

    /// A portal that answers for another interface only, until [`add_shortcuts`](Self::add_shortcuts).
    pub fn start_without_shortcuts(bus: &PrivateBus) -> Self {
        let connection = zbus::block_on(async {
            zbus::connection::Builder::address(bus.address.as_str())
                .unwrap()
                .name("org.freedesktop.portal.Desktop")
                .unwrap()
                .serve_at(PORTAL_PATH, Settings)
                .unwrap()
                .build()
                .await
                .unwrap()
        });
        FakePortal {
            connection,
            state: Arc::new(State::default()),
        }
    }

    /// The portal gets its shortcuts interface, in `version`: its backend came up.
    pub fn add_shortcuts(&self, version: u32) {
        let shortcuts = Shortcuts {
            version,
            state: Arc::clone(&self.state),
        };
        zbus::block_on(self.connection.object_server().at(PORTAL_PATH, shortcuts)).expect("the interface is served");
    }

    /// Answers the bind that [`BindAnswer::Hold`] kept open, as if the user accepted the dialog.
    pub fn release_held(&self) {
        let held = self.state.held.lock().unwrap().take().expect("a held bind");
        held();
    }

    pub fn answer_binds_with(&self, answer: BindAnswer) {
        *self.state.answer.lock().unwrap() = answer;
    }

    fn session(&self) -> OwnedObjectPath {
        let session = self
            .state
            .session
            .lock()
            .unwrap()
            .clone()
            .expect("a session was created");
        OwnedObjectPath::try_from(session).unwrap()
    }

    fn emit(&self, path: &str, interface: &str, signal: &str, body: &(impl Serialize + Type)) {
        zbus::block_on(self.connection.emit_signal(None::<&str>, path, interface, signal, body))
            .expect("the signal goes out");
    }

    /// A key went down (`true`) or up (`false`).
    pub fn press(&self, id: &str, down: bool) {
        let signal = if down { "Activated" } else { "Deactivated" };
        self.emit(
            PORTAL_PATH,
            SHORTCUTS,
            signal,
            &(self.session(), id, 1_u64, HashMap::<String, OwnedValue>::new()),
        );
    }

    /// The user changed some keys in the system settings.
    pub fn keys_changed(&self, keys: &[(&str, &str)]) {
        let shortcuts: Vec<ShortcutOut> = keys.iter().map(|(id, trigger)| shortcut_out(id, trigger)).collect();
        self.emit(PORTAL_PATH, SHORTCUTS, "ShortcutsChanged", &(self.session(), shortcuts));
    }

    /// The desktop ends the session.
    pub fn close_session(&self) {
        let session = self.session();
        self.emit(
            session.as_str(),
            "org.freedesktop.portal.Session",
            "Closed",
            &(HashMap::<String, OwnedValue>::new(),),
        );
    }
}
