//! Which processes use Core Audio, and which app each one belongs to.
//!
//! Apps play audio from helper processes: Safari from its WebKit GPU process, Chrome and Discord from helpers inside
//! their bundle. macOS knows which app is responsible for each process, so the helpers are grouped under that app's
//! process. Without that function, a helper is grouped by the outermost `.app` bundle it lives in, and a process
//! outside any bundle is an app of its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::AudioApp;

/// A process that uses Core Audio, as far as grouping needs to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AudioProcess {
    pub pid: u32,
    /// Its executable.
    pub path: Option<PathBuf>,
    /// The process macOS holds responsible for it, if the function that tells is there.
    pub responsible: Option<u32>,
    /// Whether it plays audio right now.
    pub playing: bool,
}

/// An app and the processes that make its audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Group {
    pub app: AudioApp,
    /// Indexes into the processes the group was made from.
    pub members: Vec<usize>,
    /// Whether any of them plays audio right now.
    pub playing: bool,
}

impl Group {
    /// Whether the app list shows it: a real app (a bundle), or anything that plays right now. Daemons that once opened
    /// Core Audio stay out until they play.
    pub(crate) fn listed(&self) -> bool {
        self.playing || self.app.exe_path.as_deref().is_some_and(is_bundle)
    }
}

/// How processes are told apart into apps.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Pid(u32),
    Bundle(PathBuf),
}

/// Groups `processes` into apps, sorted by name. `own_pid` (this process) is left out, and so is any app it is
/// responsible for. Duplicate PIDs count once. `path_of` gives the executable of a responsible process that isn't in
/// `processes` itself (Safari is responsible for its GPU process but doesn't open Core Audio); `name_of` names an
/// app by its bundle or executable.
pub(crate) fn group(
    processes: &[AudioProcess],
    own_pid: u32,
    path_of: &dyn Fn(u32) -> Option<PathBuf>,
    name_of: &dyn Fn(&Path) -> String,
) -> Vec<Group> {
    let mut groups: Vec<(Key, Vec<usize>)> = Vec::new();
    let mut by_key: HashMap<Key, usize> = HashMap::new();
    let mut seen = Vec::new();
    for (index, process) in processes.iter().enumerate() {
        if process.pid == own_pid || seen.contains(&process.pid) {
            continue;
        }
        seen.push(process.pid);
        let key = key(process);
        if key == Key::Pid(own_pid) {
            continue;
        }
        match by_key.get(&key) {
            Some(&group) => groups[group].1.push(index),
            None => {
                by_key.insert(key.clone(), groups.len());
                groups.push((key, vec![index]));
            }
        }
    }

    let mut groups: Vec<Group> = groups
        .into_iter()
        .map(|(key, members)| {
            let member = |pid: u32| members.iter().map(|&i| &processes[i]).find(|p| p.pid == pid);
            let (pid, exe_path) = match key {
                Key::Pid(pid) => {
                    let exe = member(pid).and_then(|p| p.path.clone()).or_else(|| path_of(pid));
                    (
                        pid,
                        exe.map(|exe| outermost_app(&exe).map_or(exe.clone(), Path::to_path_buf)),
                    )
                }
                Key::Bundle(bundle) => (bundle_pid(&bundle, &members, processes), Some(bundle)),
            };
            let name = exe_path
                .as_deref()
                .map(name_of)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("PID {pid}"));
            let playing = members.iter().any(|&i| processes[i].playing);
            Group {
                app: AudioApp { pid, name, exe_path },
                members,
                playing,
            }
        })
        .collect();
    groups.sort_by_cached_key(|group| (group.app.name.to_lowercase(), group.app.pid));
    groups
}

/// The processes of the app with PID `pid`, from [`group`]. An app made by hand from a PID that groups under another
/// one (a helper, or a tool started from Terminal) is just that process. Empty if none of them uses Core Audio.
pub(crate) fn members_of(groups: &[Group], processes: &[AudioProcess], pid: u32) -> Vec<usize> {
    if let Some(group) = groups.iter().find(|group| group.app.pid == pid) {
        return group.members.clone();
    }
    groups
        .iter()
        .flat_map(|group| &group.members)
        .copied()
        .filter(|&i| processes[i].pid == pid)
        .collect()
}

/// What `process` groups under: the process responsible for it, else the outermost bundle it lives in, else itself.
fn key(process: &AudioProcess) -> Key {
    if let Some(responsible) = process.responsible.filter(|&pid| pid > 0) {
        return Key::Pid(responsible);
    }
    match process.path.as_deref().and_then(outermost_app) {
        Some(bundle) => Key::Bundle(bundle.to_path_buf()),
        None => Key::Pid(process.pid),
    }
}

/// The PID of an app grouped by its bundle: the process running the bundle's own executable, else the oldest member
/// (helpers start after their app).
fn bundle_pid(bundle: &Path, members: &[usize], processes: &[AudioProcess]) -> u32 {
    let main = bundle.join("Contents/MacOS");
    let members = members.iter().map(|&i| &processes[i]);
    members
        .clone()
        .find(|p| p.path.as_deref().and_then(Path::parent) == Some(main.as_path()))
        .or_else(|| members.min_by_key(|p| p.pid))
        .map_or(0, |p| p.pid)
}

/// The outermost `.app` bundle `path` is in, or is: `/Applications/Discord.app` for
/// `/Applications/Discord.app/Contents/Frameworks/Discord Helper.app/Contents/MacOS/Discord Helper`.
pub(crate) fn outermost_app(path: &Path) -> Option<&Path> {
    path.ancestors().filter(|p| is_bundle(p)).last()
}

fn is_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: u32 = 999;

    fn process(pid: u32, path: &str, responsible: Option<u32>) -> AudioProcess {
        AudioProcess {
            pid,
            path: Some(path.into()),
            responsible,
            playing: true,
        }
    }

    /// Names an app after its bundle without `.app`, or after its file, like Finder does.
    fn name_of(path: &Path) -> String {
        let name = if is_bundle(path) {
            path.file_stem()
        } else {
            path.file_name()
        };
        name.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn no_paths(_: u32) -> Option<PathBuf> {
        None
    }

    fn apps(groups: &[Group]) -> Vec<(u32, &str, Option<&str>)> {
        groups
            .iter()
            .map(|g| {
                (
                    g.app.pid,
                    g.app.name.as_str(),
                    g.app.exe_path.as_deref().and_then(Path::to_str),
                )
            })
            .collect()
    }

    const DISCORD: &str = "/Applications/Discord.app/Contents/MacOS/Discord";
    const DISCORD_HELPER: &str = "/Applications/Discord.app/Contents/Frameworks/Discord Helper (Renderer).app/Contents/\
                                  MacOS/Discord Helper (Renderer)";
    const WEBKIT_GPU: &str = "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/\
                              com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU";
    const SAFARI: &str = "/Applications/Safari.app/Contents/MacOS/Safari";

    #[test]
    fn discord_helpers_group_under_discord() {
        let processes = [
            process(100, DISCORD, Some(100)),
            process(101, DISCORD_HELPER, Some(100)),
        ];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        assert_eq!(apps(&groups), [(100, "Discord", Some("/Applications/Discord.app"))]);
        assert_eq!(groups[0].members, [0, 1]);
    }

    #[test]
    fn safaris_webkit_process_groups_under_safari_through_the_responsible_pid() {
        // Safari itself doesn't use Core Audio, only its GPU process, which lives outside Safari's bundle.
        let processes = [process(300, WEBKIT_GPU, Some(200))];
        let path_of = |pid| (pid == 200).then(|| PathBuf::from(SAFARI));
        let groups = group(&processes, OWN, &path_of, &name_of);
        assert_eq!(apps(&groups), [(200, "Safari", Some("/Applications/Safari.app"))]);
        assert_eq!(groups[0].members, [0]);
    }

    #[test]
    fn without_the_responsibility_function_helpers_group_by_their_outermost_app() {
        let processes = [
            process(101, DISCORD_HELPER, None),
            process(100, DISCORD, None),
            process(300, WEBKIT_GPU, None),
        ];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        // The GPU process isn't in any bundle, so it can't be told apart from Safari: it shows up on its own.
        assert_eq!(
            apps(&groups),
            [
                (300, "com.apple.WebKit.GPU", Some(WEBKIT_GPU)),
                (100, "Discord", Some("/Applications/Discord.app")),
            ]
        );
        assert_eq!(groups[1].members, [0, 1]);
    }

    #[test]
    fn a_bundle_without_its_main_process_is_named_after_its_oldest_helper() {
        let processes = [process(105, DISCORD_HELPER, None), process(102, DISCORD_HELPER, None)];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        assert_eq!(apps(&groups), [(102, "Discord", Some("/Applications/Discord.app"))]);
    }

    #[test]
    fn a_process_without_a_bundle_is_its_own_app() {
        let processes = [
            process(400, "/usr/bin/afplay", None),
            process(401, "/usr/bin/afplay", Some(401)),
        ];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        assert_eq!(
            apps(&groups),
            [
                (400, "afplay", Some("/usr/bin/afplay")),
                (401, "afplay", Some("/usr/bin/afplay"))
            ]
        );
    }

    #[test]
    fn our_own_process_and_its_helpers_are_left_out() {
        let ours = "/Applications/Repeat That.app/Contents/MacOS/repeat-that";
        let processes = [
            process(OWN, ours, Some(OWN)),
            process(1000, "/Applications/Repeat That.app/Contents/MacOS/helper", Some(OWN)),
            process(400, "/usr/bin/afplay", Some(400)),
        ];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        assert_eq!(apps(&groups), [(400, "afplay", Some("/usr/bin/afplay"))]);
    }

    #[test]
    fn started_from_a_terminal_we_share_its_group_but_stay_out_of_it() {
        // A tool started from Terminal, like the tests or the probe: Terminal is responsible for it and for the player
        // it starts. The player is listed under Terminal, without us.
        let terminal = "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal";
        let processes = [
            process(OWN, "/tmp/probe", Some(50)),
            process(400, "/usr/bin/afplay", Some(50)),
        ];
        let path_of = |pid| (pid == 50).then(|| PathBuf::from(terminal));
        let groups = group(&processes, OWN, &path_of, &name_of);
        assert_eq!(
            apps(&groups),
            [(50, "Terminal", Some("/System/Applications/Utilities/Terminal.app"))]
        );
        assert_eq!(groups[0].members, [1]);
    }

    #[test]
    fn duplicate_pids_count_once() {
        let processes = [
            process(100, DISCORD, Some(100)),
            process(100, DISCORD, Some(100)),
            process(101, DISCORD_HELPER, Some(100)),
        ];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members, [0, 2]);
    }

    #[test]
    fn an_app_without_a_path_is_named_by_its_pid() {
        let processes = [AudioProcess {
            pid: 77,
            path: None,
            responsible: None,
            playing: true,
        }];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        assert_eq!(apps(&groups), [(77, "PID 77", None)]);
    }

    #[test]
    fn silent_processes_outside_a_bundle_are_not_listed() {
        let mut daemon = process(60, "/usr/libexec/somed", None);
        daemon.playing = false;
        let mut discord = process(100, DISCORD, Some(100));
        discord.playing = false;
        let groups = group(
            &[daemon, discord, process(400, "/usr/bin/afplay", None)],
            OWN,
            &no_paths,
            &name_of,
        );
        let listed: Vec<_> = groups.iter().filter(|g| g.listed()).map(|g| g.app.pid).collect();
        assert_eq!(listed, [400, 100]);
    }

    #[test]
    fn members_of_an_app_or_of_a_process_in_one() {
        let processes = [
            process(100, DISCORD, Some(100)),
            process(101, DISCORD_HELPER, Some(100)),
            process(400, "/usr/bin/afplay", Some(50)),
            process(401, "/usr/bin/afplay", Some(50)),
        ];
        let groups = group(&processes, OWN, &no_paths, &name_of);
        assert_eq!(members_of(&groups, &processes, 100), [0, 1]);
        assert_eq!(members_of(&groups, &processes, 50), [2, 3]);
        // A PID given by hand that groups under another one: just that process.
        assert_eq!(members_of(&groups, &processes, 401), [3]);
        assert!(members_of(&groups, &processes, 12345).is_empty());
    }

    #[test]
    fn the_outermost_app_of_a_path() {
        let outer = |p: &str| outermost_app(Path::new(p)).map(|p| p.to_str().unwrap().to_owned());
        assert_eq!(outer(DISCORD).as_deref(), Some("/Applications/Discord.app"));
        assert_eq!(outer(DISCORD_HELPER).as_deref(), Some("/Applications/Discord.app"));
        assert_eq!(
            outer("/Applications/Discord.app").as_deref(),
            Some("/Applications/Discord.app")
        );
        assert_eq!(
            outer("/Users/me/Applications/Chrome Apps.localized/YouTube.app/Contents/MacOS/app_mode_loader").as_deref(),
            Some("/Users/me/Applications/Chrome Apps.localized/YouTube.app")
        );
        assert_eq!(
            outer("/Applications/UPPER.APP/Contents/MacOS/x").as_deref(),
            Some("/Applications/UPPER.APP")
        );
        assert_eq!(outer("/usr/bin/afplay"), None);
        assert_eq!(outer(WEBKIT_GPU), None);
        assert_eq!(outer("/Applications/Not an app.apps/x"), None);
        assert_eq!(outer(""), None);
    }
}
