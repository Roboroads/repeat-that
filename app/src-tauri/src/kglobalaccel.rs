//! The name KDE Plasma shows above the app's shortcuts in System Settings → Shortcuts.
//!
//! Plasma's portal keeps the shortcuts in kglobalaccel, as a component named after the app ID, and kglobalaccel keeps a
//! display name next to it, which the Shortcuts page shows. The portal of Plasma 6.3 (Debian 13 has 6.3.5) sets that
//! name to the app ID every time a session starts: `loadActions` registers the shortcuts it already has with
//! `componentDisplayName` set to the component name, and the later `BindShortcuts` reuses those registered actions, so
//! the name from the desktop file it looks up there never reaches kglobalaccel. The page then lists the shortcuts under
//! `me.roboroads.repeat-that`. Plasma 6.4 reads the name from the desktop file in both places (xdg-desktop-portal-kde
//! commit 2fd2d093, "Improve the global shortcuts workflow").
//!
//! So after the first bind of each session the app gives the component its name back through kglobalaccel's own
//! `doRegister`, which updates the display name of an action that exists. It only touches a component that has actions,
//! so it never adds one, and does nothing where kglobalaccel isn't running (GNOME and the rest) or the name is anything
//! but the app ID (Plasma 6.4 on, which also keeps a translated name from the desktop file).

use zbus::Connection;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;

const SERVICE: &str = "org.kde.kglobalaccel";
const PATH: &str = "/kglobalaccel";
const INTERFACE: &str = "org.kde.KGlobalAccel";

/// The name the Shortcuts page should show, the `Name` of the desktop file.
pub const APP_NAME: &str = "Repeat That";

/// Gives `component` the display name `name` in kglobalaccel when its display name is its own unique name. `Ok(true)`
/// when it was and now has `name`, `Ok(false)` when there was nothing to do: no kglobalaccel on the bus, no shortcuts in
/// the component yet, or a real name already (`name`, or one the desktop chose, such as a translation).
pub async fn name_component(connection: &Connection, component: &str, name: &str) -> Result<bool, String> {
    // Asked first, so that a call can't start kglobalaccel through D-Bus activation on a desktop that doesn't run it.
    let bus = DBusProxy::new(connection).await.map_err(|err| err.to_string())?;
    let service = BusName::try_from(SERVICE).map_err(|err| err.to_string())?;
    if !bus.name_has_owner(service).await.map_err(|err| err.to_string())? {
        return Ok(false);
    }

    // Each action as kglobalaccel's action ID: component, action, component display name, action display name.
    let query = vec![component.to_string(), String::new(), String::new(), String::new()];
    let reply = connection
        .call_method(
            Some(SERVICE),
            PATH,
            Some(INTERFACE),
            "allActionsForComponent",
            &(query,),
        )
        .await
        .map_err(|err| err.to_string())?;
    let actions: Vec<Vec<String>> = reply.body().deserialize().map_err(|err| err.to_string())?;
    let Some(action) = actions.into_iter().find(|id| id.len() == 4) else {
        return Ok(false);
    };
    // Only the name the 6.3 portal leaves is replaced. Any other is the desktop's own, a translated one among them.
    if action[2] != component {
        return Ok(false);
    }

    // The display name belongs to the component, so one action carries it for all. The action keeps its own name.
    let id = vec![
        component.to_string(),
        action[1].clone(),
        name.to_string(),
        action[3].clone(),
    ];
    connection
        .call_method(Some(SERVICE), PATH, Some(INTERFACE), "doRegister", &(id,))
        .await
        .map_err(|err| err.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::private_bus::PrivateBus;

    const ID: &str = "me.roboroads.repeat-that";

    /// Each action with its display name.
    type Actions = Vec<(String, String)>;
    /// A component: its unique name, its display name and its actions.
    type Component = (String, String, Actions);
    /// A component the daemon knows at the start, as `Component`.
    type Known<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

    /// kglobalaccel as far as these calls go: components with a display name and actions, and `doRegister` doing what
    /// the daemon does with an action it knows (a new display name for its component, a new one for the action when one
    /// is given) or one it doesn't (a new action).
    #[derive(Default)]
    struct Daemon {
        components: Mutex<Vec<Component>>,
        registered: Mutex<Vec<Vec<String>>>,
    }

    struct Fake(Arc<Daemon>);

    #[zbus::interface(name = "org.kde.KGlobalAccel")]
    impl Fake {
        #[zbus(name = "allActionsForComponent")]
        fn all_actions_for_component(&self, id: Vec<String>) -> Vec<Vec<String>> {
            let components = self.0.components.lock().unwrap();
            let Some((component, name, actions)) = components.iter().find(|(component, ..)| *component == id[0]) else {
                return Vec::new();
            };
            actions
                .iter()
                .map(|(action, text)| vec![component.clone(), action.clone(), name.clone(), text.clone()])
                .collect()
        }

        #[zbus(name = "doRegister")]
        fn do_register(&self, id: Vec<String>) {
            self.0.registered.lock().unwrap().push(id.clone());
            let mut components = self.0.components.lock().unwrap();
            if let Some((_, name, actions)) = components.iter_mut().find(|(component, ..)| *component == id[0]) {
                if !id[2].is_empty() {
                    name.clone_from(&id[2]);
                }
                match actions.iter_mut().find(|(action, _)| *action == id[1]) {
                    Some((_, text)) if !id[3].is_empty() => text.clone_from(&id[3]),
                    Some(_) => {}
                    None => actions.push((id[1].clone(), id[3].clone())),
                }
            }
        }
    }

    struct Rig {
        daemon: Arc<Daemon>,
        client: Connection,
        _server: Option<Connection>,
        // Last, so the daemon outlives everything that talks to it.
        _bus: PrivateBus,
    }

    impl Rig {
        /// A bus with kglobalaccel on it when `running`, knowing `components`. `None` without `dbus-daemon`.
        fn new(running: bool, components: &[Known<'_>]) -> Option<Rig> {
            let bus = PrivateBus::start()?;
            let daemon = Arc::new(Daemon::default());
            *daemon.components.lock().unwrap() = components
                .iter()
                .map(|(component, name, actions)| {
                    let actions = actions.iter().map(|(a, t)| (a.to_string(), t.to_string())).collect();
                    (component.to_string(), name.to_string(), actions)
                })
                .collect();
            let server = running.then(|| {
                zbus::block_on(async {
                    zbus::connection::Builder::address(bus.address.as_str())
                        .unwrap()
                        .name(SERVICE)
                        .unwrap()
                        .serve_at(PATH, Fake(Arc::clone(&daemon)))
                        .unwrap()
                        .build()
                        .await
                        .unwrap()
                })
            });
            let client =
                zbus::block_on(async { zbus::connection::Builder::address(bus.address.as_str())?.build().await })
                    .unwrap();
            Some(Rig {
                daemon,
                client,
                _server: server,
                _bus: bus,
            })
        }

        fn name(&self) -> Result<bool, String> {
            zbus::block_on(name_component(&self.client, ID, APP_NAME))
        }

        fn display_name(&self) -> String {
            self.daemon.components.lock().unwrap()[0].1.clone()
        }

        fn actions(&self) -> Actions {
            self.daemon.components.lock().unwrap()[0].2.clone()
        }
    }

    const ACTIONS: &[(&str, &str)] = &[("replay", "Replay"), ("stop", "Stop"), ("catch-up", "Catch up")];

    #[test]
    fn a_component_named_after_the_app_id_gets_the_apps_name() {
        let Some(rig) = Rig::new(true, &[(ID, ID, ACTIONS)]) else {
            return;
        };
        assert_eq!(rig.name(), Ok(true));
        assert_eq!(rig.display_name(), APP_NAME);
        let registered = rig.daemon.registered.lock().unwrap().clone();
        assert_eq!(registered, [vec![ID, "replay", APP_NAME, "Replay"]]);
    }

    #[test]
    fn the_actions_keep_their_names_and_none_is_added() {
        let Some(rig) = Rig::new(true, &[(ID, ID, ACTIONS)]) else {
            return;
        };
        rig.name().unwrap();
        let expected: Actions = ACTIONS.iter().map(|(a, t)| (a.to_string(), t.to_string())).collect();
        assert_eq!(rig.actions(), expected);
    }

    #[test]
    fn a_component_with_the_right_name_is_left_alone() {
        let Some(rig) = Rig::new(true, &[(ID, APP_NAME, ACTIONS)]) else {
            return;
        };
        assert_eq!(rig.name(), Ok(false));
        assert!(rig.daemon.registered.lock().unwrap().is_empty());
    }

    #[test]
    fn a_name_the_desktop_chose_is_kept() {
        let Some(rig) = Rig::new(true, &[(ID, "Herhaal dat", ACTIONS)]) else {
            return;
        };
        assert_eq!(rig.name(), Ok(false));
        assert_eq!(rig.display_name(), "Herhaal dat");
        assert!(rig.daemon.registered.lock().unwrap().is_empty());
    }

    #[test]
    fn a_component_without_shortcuts_is_not_given_one() {
        let Some(rig) = Rig::new(true, &[(ID, ID, &[])]) else {
            return;
        };
        assert_eq!(rig.name(), Ok(false));
        assert!(rig.daemon.registered.lock().unwrap().is_empty());
        assert!(rig.actions().is_empty());
    }

    #[test]
    fn an_unknown_component_is_not_made() {
        let Some(rig) = Rig::new(true, &[("org.kde.konsole", "Konsole", ACTIONS)]) else {
            return;
        };
        assert_eq!(rig.name(), Ok(false));
        assert!(rig.daemon.registered.lock().unwrap().is_empty());
    }

    #[test]
    fn without_kglobalaccel_there_is_nothing_to_do() {
        let Some(rig) = Rig::new(false, &[(ID, ID, ACTIONS)]) else {
            return;
        };
        assert_eq!(rig.name(), Ok(false));
        assert_eq!(rig.display_name(), ID);
    }
}
