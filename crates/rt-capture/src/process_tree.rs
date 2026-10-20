//! Grouping processes into apps. Discord, Teams and browsers play audio from child processes, so the app a user picks
//! is a tree of processes, identified by its top-most process.

// Only the Windows backend uses this so far; the tests run everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashMap;

/// Helper processes that belong to the app that started them, even though their executable is different (Teams plays
/// its audio from WebView2).
const HELPER_EXES: &[&str] = &["msedgewebview2.exe"];

/// Guards against parent loops, which PID reuse can create.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone)]
pub(crate) struct Process {
    pub parent: u32,
    /// Executable file name, e.g. `Discord.exe`.
    pub exe: String,
}

#[derive(Debug, Default)]
pub(crate) struct ProcessTable(pub HashMap<u32, Process>);

impl ProcessTable {
    /// The top-most process of the app `pid` belongs to: walks up through parents with the same executable, and from
    /// helpers like WebView2 to the process that started them.
    pub fn app_root(&self, pid: u32) -> u32 {
        let mut current = pid;
        for _ in 0..MAX_DEPTH {
            let Some(process) = self.0.get(&current) else {
                break;
            };
            let Some(parent) = self.0.get(&process.parent).filter(|_| process.parent != current) else {
                break;
            };
            let same_exe = parent.exe.eq_ignore_ascii_case(&process.exe);
            let helper = HELPER_EXES.iter().any(|h| process.exe.eq_ignore_ascii_case(h));
            if !same_exe && !helper {
                break;
            }
            current = process.parent;
        }
        current
    }

    /// Whether `pid` is `root` or one of its descendants.
    pub fn is_in_tree(&self, pid: u32, root: u32) -> bool {
        let mut current = pid;
        for _ in 0..MAX_DEPTH {
            if current == root {
                return true;
            }
            match self.0.get(&current) {
                Some(process) if process.parent != current => current = process.parent,
                _ => return false,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[(u32, u32, &str)]) -> ProcessTable {
        ProcessTable(
            entries
                .iter()
                .map(|&(pid, parent, exe)| {
                    (
                        pid,
                        Process {
                            parent,
                            exe: exe.into(),
                        },
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn walks_up_to_the_top_process_with_the_same_exe() {
        let t = table(&[
            (1, 0, "explorer.exe"),
            (10, 1, "Discord.exe"),
            (11, 10, "Discord.exe"),
            (12, 11, "discord.exe"),
        ]);
        assert_eq!(t.app_root(12), 10);
        assert_eq!(t.app_root(10), 10);
    }

    #[test]
    fn webview_belongs_to_the_app_that_started_it() {
        let t = table(&[
            (1, 0, "explorer.exe"),
            (20, 1, "ms-teams.exe"),
            (21, 20, "msedgewebview2.exe"),
            (22, 21, "msedgewebview2.exe"),
        ]);
        assert_eq!(t.app_root(22), 20);
    }

    #[test]
    fn stops_at_an_unrelated_parent() {
        let t = table(&[(1, 0, "explorer.exe"), (30, 1, "firefox.exe")]);
        assert_eq!(t.app_root(30), 30);
        // Unknown processes are their own app.
        assert_eq!(t.app_root(99), 99);
    }

    #[test]
    fn survives_parent_loops() {
        let t = table(&[(40, 41, "a.exe"), (41, 40, "a.exe"), (50, 50, "b.exe")]);
        let root = t.app_root(40);
        assert!(root == 40 || root == 41);
        assert_eq!(t.app_root(50), 50);
        assert!(!t.is_in_tree(40, 7));
    }

    #[test]
    fn tree_membership() {
        let t = table(&[
            (1, 0, "explorer.exe"),
            (20, 1, "ms-teams.exe"),
            (21, 20, "msedgewebview2.exe"),
        ]);
        assert!(t.is_in_tree(21, 20));
        assert!(t.is_in_tree(20, 20));
        assert!(!t.is_in_tree(1, 20));
        assert!(!t.is_in_tree(99, 20));
    }
}
