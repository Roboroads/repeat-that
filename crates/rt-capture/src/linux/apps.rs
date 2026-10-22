//! Which app a PipeWire playback stream belongs to, and the app list made from that.
//!
//! Every `Stream/Output/Audio` node is a stream some app plays. Where `/proc` shows the stream's process, the app is the
//! process tree it belongs to, the same way as on Windows (see [`ProcessTable::app_root`]): two instances of one app are
//! two apps, and the app's executable identifies it across restarts. Inside a Flatpak, or for a PulseAudio app inside
//! one, `/proc` doesn't show it, and the app is every stream with the same binary, or the same name.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use super::file_name;
use crate::AudioApp;
use crate::process_tree::{Process, ProcessTable};

/// What PipeWire says about a playback stream and the client that owns it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StreamInfo {
    /// `pipewire.sec.pid` of the client: the PID the daemon saw on the socket. It is a host PID even for an app in a
    /// sandbox, but for a PulseAudio app it is pipewire-pulse, which plays on its behalf.
    pub client_pid: Option<u32>,
    /// `application.process.id`, as the app itself says. An app in a sandbox gives a PID of its own namespace.
    pub app_pid: Option<u32>,
    /// Whether the stream comes through pipewire-pulse (`client.api`).
    pub pulse: bool,
    /// `application.process.binary`, e.g. `firefox-esr`.
    pub binary: Option<String>,
    /// `application.name`, e.g. `Firefox`.
    pub name: Option<String>,
}

/// A process as `/proc` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProcInfo {
    pub parent: u32,
    pub exe: PathBuf,
}

/// Looks processes up, in `/proc` or, in tests, in a table.
pub(super) trait Procs {
    /// The process `pid`, or `None` if it isn't there or can't be seen.
    fn get(&self, pid: u32) -> Option<ProcInfo>;
}

/// The processes in `/proc`.
pub(super) struct ProcFs;

impl Procs for ProcFs {
    fn get(&self, pid: u32) -> Option<ProcInfo> {
        let dir = PathBuf::from(format!("/proc/{pid}"));
        // The command name in `stat` is in parentheses and can contain anything, so the fields after it are found from
        // the last `)`: state, then the parent PID.
        let stat = fs::read_to_string(dir.join("stat")).ok()?;
        let parent = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
        let exe = fs::read_link(dir.join("exe")).ok()?;
        // An executable replaced by an update since the process started.
        let exe = match exe.to_str().and_then(|e| e.strip_suffix(" (deleted)")) {
            Some(path) => PathBuf::from(path),
            None => exe,
        };
        Some(ProcInfo { parent, exe })
    }
}

/// Which app a stream belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum AppKey {
    /// The top-most process of the app, where `/proc` shows it.
    Process(u32),
    /// `application.process.binary`, where it doesn't.
    Binary(String),
    /// `application.name`, for a stream without a binary either.
    Name(String),
}

impl AppKey {
    /// The app's process on this system, if `/proc` showed it. A PID a stream gives otherwise can be one of a sandbox,
    /// which is someone else here, or no one.
    pub fn process(&self) -> Option<u32> {
        match self {
            AppKey::Process(pid) => Some(*pid),
            AppKey::Binary(_) | AppKey::Name(_) => None,
        }
    }
}

/// Where a stream belongs: its app, and the app's process and executable where they are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Placement {
    pub key: AppKey,
    /// The top-most process of the app, or the PID the stream gives where `/proc` doesn't show it.
    pub pid: Option<u32>,
    pub exe: Option<PathBuf>,
}

/// The PID a stream plays from, as far as PipeWire knows. For a PulseAudio app that is the PID the app gives, because
/// the client is pipewire-pulse.
pub(super) fn stream_pid(stream: &StreamInfo) -> Option<u32> {
    if stream.pulse {
        stream.app_pid
    } else {
        stream.client_pid.or(stream.app_pid)
    }
}

/// Places `stream` in its app. `None` for a stream that names neither a process, a binary nor an app.
pub(super) fn place(stream: &StreamInfo, procs: &impl Procs) -> Option<Placement> {
    let pid = stream_pid(stream);
    if let Some(pid) = pid
        && let Some(process) = procs.get(pid)
        // A PulseAudio app gives its own PID, which in a sandbox is one of the sandbox: it only counts if it is the
        // binary the stream names.
        && (!stream.pulse || stream.binary.as_deref().is_none_or(|b| file_name(&process.exe) == b))
    {
        let (root, exe) = app_root(pid, process, procs);
        return Some(Placement {
            key: AppKey::Process(root),
            pid: Some(root),
            exe: Some(exe),
        });
    }
    let key = match (&stream.binary, &stream.name) {
        (Some(binary), _) => AppKey::Binary(binary.clone()),
        (None, Some(name)) => AppKey::Name(name.clone()),
        (None, None) => return None,
    };
    Some(Placement { key, pid, exe: None })
}

/// The top-most process of the app `pid` (whose `/proc` entry is `process`) belongs to, and its executable.
fn app_root(pid: u32, process: ProcInfo, procs: &impl Procs) -> (u32, PathBuf) {
    // Only the ancestors `app_root` can walk through are read: they share the executable's name.
    let mut table = ProcessTable::default();
    let mut exes = BTreeMap::new();
    let (mut current, mut info) = (pid, process);
    for _ in 0..64 {
        let parent = info.parent;
        let name = file_name(&info.exe).to_owned();
        table.0.insert(
            current,
            Process {
                parent,
                exe: name.clone(),
            },
        );
        exes.insert(current, info.exe);
        if parent == 0 || parent == current || table.0.contains_key(&parent) {
            break;
        }
        match procs.get(parent) {
            Some(next) if file_name(&next.exe).eq_ignore_ascii_case(&name) => (current, info) = (parent, next),
            _ => break,
        }
    }
    let root = table.app_root(pid);
    let exe = exes.remove(&root).unwrap_or_default();
    (root, exe)
}

/// Our own app: streams of this process tree (the replay output, say) aren't listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Own {
    pub pid: u32,
    pub root: u32,
}

impl Own {
    pub fn new(procs: &impl Procs) -> Self {
        Self::new_for(std::process::id(), procs)
    }

    fn new_for(pid: u32, procs: &impl Procs) -> Self {
        let root = procs.get(pid).map_or(pid, |process| app_root(pid, process, procs).0);
        Self { pid, root }
    }

    fn owns(&self, stream: &StreamInfo, placement: &Placement) -> bool {
        stream_pid(stream) == Some(self.pid) || placement.key == AppKey::Process(self.root)
    }
}

/// A playback stream, placed.
#[derive(Debug, Clone)]
pub(super) struct Placed {
    pub node: u32,
    pub info: StreamInfo,
    pub placement: Placement,
}

/// An app in the list, and its streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AppGroup {
    pub app: AudioApp,
    pub key: AppKey,
    /// Node ids, in ascending order.
    pub nodes: Vec<u32>,
}

/// The apps `streams` belong to, without our own, sorted by name. `desktop_name` gives the name from the `.desktop`
/// file of an executable or binary name, if one matches.
pub(super) fn group(streams: &[Placed], own: Own, desktop_name: impl Fn(&str) -> Option<String>) -> Vec<AppGroup> {
    let mut groups: BTreeMap<&AppKey, Vec<&Placed>> = BTreeMap::new();
    for stream in streams {
        if !own.owns(&stream.info, &stream.placement) {
            groups.entry(&stream.placement.key).or_default().push(stream);
        }
    }
    let mut apps: Vec<AppGroup> = groups
        .into_iter()
        .map(|(key, mut streams)| {
            streams.sort_by_key(|s| s.node);
            let first = streams[0];
            let pid = streams.iter().find_map(|s| s.placement.pid);
            let exe = first.placement.exe.clone();
            let program = exe
                .as_deref()
                .map(|exe| file_name(exe).to_owned())
                .or_else(|| first.info.binary.clone());
            let name = program
                .as_deref()
                .and_then(&desktop_name)
                .or_else(|| streams.iter().find_map(|s| s.info.name.clone()))
                .or(program)
                .unwrap_or_else(|| format!("PID {}", pid.unwrap_or(0)));
            AppGroup {
                app: AudioApp {
                    pid: pid.unwrap_or(0),
                    name,
                    exe_path: exe,
                },
                key: key.clone(),
                nodes: streams.iter().map(|s| s.node).collect(),
            }
        })
        .collect();
    apps.sort_by_cached_key(|group| (group.app.name.to_lowercase(), group.app.pid));
    apps
}

/// The group `app` (from an earlier list) is now: the same app, else one with its executable, else one whose
/// process it is, else one with its name.
pub(super) fn find<'a>(groups: &'a [AppGroup], app: &AudioApp) -> Option<&'a AppGroup> {
    groups
        .iter()
        .find(|g| g.app == *app)
        .or_else(|| {
            app.exe_path
                .as_ref()
                .and_then(|exe| groups.iter().find(|g| g.app.exe_path.as_ref() == Some(exe)))
        })
        .or_else(|| groups.iter().find(|g| app.pid != 0 && g.app.pid == app.pid))
        .or_else(|| groups.iter().find(|g| g.app.name == app.name))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use super::*;

    /// A fake `/proc`: PID → (parent, executable).
    struct FakeProcs(HashMap<u32, (u32, &'static str)>);

    impl FakeProcs {
        fn new(entries: &[(u32, u32, &'static str)]) -> Self {
            Self(entries.iter().map(|&(pid, parent, exe)| (pid, (parent, exe))).collect())
        }
    }

    impl Procs for FakeProcs {
        fn get(&self, pid: u32) -> Option<ProcInfo> {
            self.0.get(&pid).map(|&(parent, exe)| ProcInfo {
                parent,
                exe: exe.into(),
            })
        }
    }

    /// A Flatpak's view: no other process is visible.
    struct NoProcs;

    impl Procs for NoProcs {
        fn get(&self, _pid: u32) -> Option<ProcInfo> {
            None
        }
    }

    const OWN: Own = Own { pid: 500, root: 500 };

    fn native(pid: u32, binary: &str, name: &str) -> StreamInfo {
        StreamInfo {
            client_pid: Some(pid),
            app_pid: Some(pid),
            pulse: false,
            binary: Some(binary.into()),
            name: Some(name.into()),
        }
    }

    fn pulse(app_pid: u32, binary: &str, name: &str) -> StreamInfo {
        StreamInfo {
            client_pid: Some(900),
            app_pid: Some(app_pid),
            pulse: true,
            binary: Some(binary.into()),
            name: Some(name.into()),
        }
    }

    fn placed(procs: &impl Procs, streams: &[(u32, StreamInfo)]) -> Vec<Placed> {
        streams
            .iter()
            .map(|(node, info)| Placed {
                node: *node,
                info: info.clone(),
                placement: place(info, procs).expect("placed"),
            })
            .collect()
    }

    fn no_desktop(_: &str) -> Option<String> {
        None
    }

    fn summary(groups: &[AppGroup]) -> Vec<(u32, &str, Option<&str>, Vec<u32>)> {
        groups
            .iter()
            .map(|g| {
                (
                    g.app.pid,
                    g.app.name.as_str(),
                    g.app.exe_path.as_deref().and_then(Path::to_str),
                    g.nodes.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn streams_of_one_process_tree_are_one_app() {
        let procs = FakeProcs::new(&[
            (1, 0, "/usr/lib/systemd/systemd"),
            (100, 1, "/usr/lib/firefox-esr/firefox-esr"),
            (101, 100, "/usr/lib/firefox-esr/firefox-esr"),
            (102, 100, "/usr/lib/firefox-esr/firefox-esr"),
        ]);
        let streams = placed(
            &procs,
            &[
                (40, pulse(101, "firefox-esr", "Firefox")),
                (41, pulse(102, "firefox-esr", "Firefox")),
            ],
        );
        assert_eq!(
            summary(&group(&streams, OWN, no_desktop)),
            [(100, "Firefox", Some("/usr/lib/firefox-esr/firefox-esr"), vec![40, 41])]
        );
    }

    #[test]
    fn the_desktop_file_names_the_app() {
        let procs = FakeProcs::new(&[(100, 1, "/usr/lib/firefox-esr/firefox-esr")]);
        let streams = placed(&procs, &[(40, pulse(100, "firefox-esr", "Firefox"))]);
        let desktop = |program: &str| (program == "firefox-esr").then(|| "Firefox ESR".to_owned());
        assert_eq!(group(&streams, OWN, desktop)[0].app.name, "Firefox ESR");
    }

    #[test]
    fn a_native_stream_takes_the_pid_the_daemon_saw() {
        // The app says another PID (it lives in a sandbox, say); the client's socket tells the truth.
        let procs = FakeProcs::new(&[(100, 1, "/usr/bin/pw-cat"), (7, 0, "/usr/bin/bash")]);
        let stream = StreamInfo {
            client_pid: Some(100),
            app_pid: Some(7),
            ..native(100, "rt-tone", "Tone")
        };
        let placement = place(&stream, &procs).unwrap();
        assert_eq!(placement.key, AppKey::Process(100));
        assert_eq!(placement.exe.as_deref(), Some(Path::new("/usr/bin/pw-cat")));
    }

    #[test]
    fn a_pulseaudio_stream_takes_the_pid_of_the_app_not_of_pipewire_pulse() {
        let procs = FakeProcs::new(&[(900, 1, "/usr/bin/pipewire-pulse"), (300, 1, "/usr/bin/paplay")]);
        let placement = place(&pulse(300, "paplay", "paplay"), &procs).unwrap();
        assert_eq!(placement.key, AppKey::Process(300));
    }

    #[test]
    fn a_pulseaudio_pid_from_a_sandbox_is_not_trusted() {
        // PID 2 inside the sandbox is someone else on the host.
        let procs = FakeProcs::new(&[(2, 0, "/usr/lib/kthreadd")]);
        let placement = place(&pulse(2, "firefox", "Firefox"), &procs).unwrap();
        assert_eq!(placement.key, AppKey::Binary("firefox".into()));
        assert_eq!(placement.exe, None);
        // Nor is it a process here that a capture could wait for.
        assert_eq!(placement.key.process(), None);
        assert_eq!(AppKey::Process(300).process(), Some(300));
    }

    #[test]
    fn without_proc_the_app_is_its_binary_or_else_its_name() {
        let by_binary = place(&native(100, "Discord", "WEBRTC VoiceEngine"), &NoProcs).unwrap();
        assert_eq!(by_binary.key, AppKey::Binary("Discord".into()));
        assert_eq!(by_binary.pid, Some(100));
        let nameless = StreamInfo {
            binary: None,
            ..native(100, "", "Tone")
        };
        assert_eq!(place(&nameless, &NoProcs).unwrap().key, AppKey::Name("Tone".into()));
        let anonymous = StreamInfo {
            binary: None,
            name: None,
            ..native(100, "", "")
        };
        assert_eq!(place(&anonymous, &NoProcs), None);
    }

    #[test]
    fn in_a_flatpak_streams_of_one_binary_are_one_app() {
        // Two Discord streams, a voice call and a notification, from PIDs the sandbox can't see.
        let streams = placed(
            &NoProcs,
            &[
                (60, native(101, "Discord", "WEBRTC VoiceEngine")),
                (61, native(102, "Discord", "Discord")),
                (62, native(103, "firefox", "Firefox")),
            ],
        );
        assert_eq!(
            summary(&group(&streams, OWN, no_desktop)),
            [
                (103, "Firefox", None, vec![62]),
                (101, "WEBRTC VoiceEngine", None, vec![60, 61]),
            ]
        );
        let desktop = |program: &str| (program == "Discord").then(|| "Discord".to_owned());
        assert_eq!(group(&streams, OWN, desktop)[0].app.name, "Discord");
    }

    #[test]
    fn two_instances_of_one_app_are_two_apps() {
        let procs = FakeProcs::new(&[
            (100, 1, "/usr/lib/firefox-esr/firefox-esr"),
            (200, 1, "/usr/lib/firefox-esr/firefox-esr"),
        ]);
        let streams = placed(
            &procs,
            &[
                (40, pulse(100, "firefox-esr", "Firefox")),
                (41, pulse(200, "firefox-esr", "Firefox")),
            ],
        );
        let groups = group(&streams, OWN, no_desktop);
        assert_eq!(
            summary(&groups),
            [
                (100, "Firefox", Some("/usr/lib/firefox-esr/firefox-esr"), vec![40]),
                (200, "Firefox", Some("/usr/lib/firefox-esr/firefox-esr"), vec![41]),
            ]
        );
        // Both have the executable a target remembers; the first one is picked.
        let target = AudioApp {
            pid: 999,
            name: "Firefox".into(),
            exe_path: Some("/usr/lib/firefox-esr/firefox-esr".into()),
        };
        assert_eq!(find(&groups, &target).map(|g| g.app.pid), Some(100));
    }

    #[test]
    fn our_own_streams_are_not_listed() {
        let procs = FakeProcs::new(&[
            (500, 1, "/usr/bin/repeat-that"),
            (501, 500, "/usr/bin/repeat-that"),
            (600, 500, "/usr/bin/pw-cat"),
        ]);
        let own = Own::new_for(500, &procs);
        let streams = placed(
            &procs,
            &[
                // The replay output, and a guard that would play.
                (70, native(500, "repeat-that", "Repeat That")),
                (71, native(501, "repeat-that", "Repeat That")),
                // A player we started is an app of its own.
                (72, native(600, "pw-cat", "Tone")),
            ],
        );
        assert_eq!(
            summary(&group(&streams, own, no_desktop)),
            [(600, "Tone", Some("/usr/bin/pw-cat"), vec![72])]
        );
        // Without /proc only our own PID is known.
        let streams = placed(&NoProcs, &[(70, native(500, "repeat-that", "Repeat That"))]);
        assert!(group(&streams, own, no_desktop).is_empty());
    }

    #[test]
    fn apps_are_sorted_by_name() {
        let streams = placed(
            &NoProcs,
            &[
                (1, native(10, "b", "beta")),
                (2, native(11, "a", "Alpha")),
                (3, native(12, "c", "gamma")),
            ],
        );
        let names: Vec<_> = group(&streams, OWN, no_desktop)
            .into_iter()
            .map(|g| g.app.name)
            .collect();
        assert_eq!(names, ["Alpha", "beta", "gamma"]);
    }

    #[test]
    fn an_app_from_an_earlier_list_is_found_again() {
        let streams = placed(&NoProcs, &[(1, native(10, "a", "Alpha")), (2, native(11, "b", "Beta"))]);
        let groups = group(&streams, OWN, no_desktop);
        assert_eq!(find(&groups, &groups[1].app).map(|g| g.nodes.clone()), Some(vec![2]));
        let renamed_pid = AudioApp {
            pid: 77,
            ..groups[0].app.clone()
        };
        assert_eq!(find(&groups, &renamed_pid).map(|g| g.nodes.clone()), Some(vec![1]));
        let gone = AudioApp {
            pid: 77,
            name: "Gamma".into(),
            exe_path: None,
        };
        assert_eq!(find(&groups, &gone), None);
    }

    #[test]
    fn proc_fs_reads_this_process() {
        let me = ProcFs.get(std::process::id()).expect("our own /proc entry");
        assert_eq!(me.exe, std::env::current_exe().unwrap());
        assert!(me.parent > 0);
    }
}
