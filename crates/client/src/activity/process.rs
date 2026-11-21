//! A snapshot of the process table, used to tell whether the focused window
//! runs an agent: the agent's process is a descendant of the window's
//! process (a terminal, or an editor's built-in terminal).
//!
//! Only process ids, parent ids, executable names, agent command lines (to
//! recognise the agent) and the agents' working directories are read. None
//! of it leaves the machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{AgentSession, FocusedApp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    /// The executable's file name, lowercased, without `.exe`.
    pub name: String,
    /// The agent this process runs ([`crate::agents::Integration::id`]).
    pub agent: Option<&'static str>,
    /// Working directory, read only for agent processes and only where the
    /// OS allows it (not on Windows).
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct Processes {
    by_pid: HashMap<u32, Process>,
}

/// How far up the tree to look. Guards against cycles from reused pids.
const MAX_DEPTH: usize = 64;

impl Processes {
    /// Read the process table now. Empty if the OS doesn't let us.
    pub fn snapshot() -> Self {
        Self::from_list(list())
    }

    pub fn from_list(list: Vec<Process>) -> Self {
        Self {
            by_pid: list.into_iter().map(|p| (p.pid, p)).collect(),
        }
    }

    pub fn get(&self, pid: u32) -> Option<&Process> {
        self.by_pid.get(&pid)
    }

    /// Processes running `session`'s agent in its project folder. Where the
    /// working directory is unknown, any process of that agent counts.
    pub fn agent_processes<'a>(
        &'a self,
        session: AgentSession<'a>,
    ) -> impl Iterator<Item = &'a Process> + 'a {
        let project = canonical(session.project);
        self.by_pid.values().filter(move |p| {
            p.agent == Some(session.agent)
                && p.cwd.as_deref().is_none_or(|cwd| canonical(cwd) == project)
        })
    }

    /// `pid` itself and its ancestors, nearest first.
    pub fn ancestors(&self, pid: u32) -> impl Iterator<Item = &Process> {
        let mut next = self.by_pid.get(&pid);
        let mut depth = 0;
        std::iter::from_fn(move || {
            let current = next?;
            depth += 1;
            next = (depth < MAX_DEPTH && current.ppid != current.pid)
                .then(|| self.by_pid.get(&current.ppid))
                .flatten();
            Some(current)
        })
    }

    /// Whether the focused window runs `session`'s agent.
    ///
    /// With the window's process id: the agent process descends from it.
    /// Without one (wlr-foreign-toplevel gives only the app id): the agent
    /// has an ancestor whose executable matches the app id, which can't tell
    /// two windows of the same terminal apart.
    pub fn window_runs_agent(&self, app: &FocusedApp, session: AgentSession<'_>) -> bool {
        self.agent_processes(session).any(|agent| {
            let mut ancestors = self.ancestors(agent.pid).skip(1);
            match app.pid {
                Some(pid) => ancestors.any(|p| p.pid == pid),
                None => {
                    let id = app_name(&app.app_id);
                    !id.is_empty() && ancestors.any(|p| p.name == id)
                }
            }
        })
    }
}

/// The last part of an app id, lowercased: `org.wezfurlong.wezterm` →
/// `wezterm`, `Alacritty` → `alacritty`.
fn app_name(app_id: &str) -> String {
    app_id.rsplit('.').next().unwrap_or(app_id).to_lowercase()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The agent a process runs, from its executable name and command line.
/// Claude Code runs as `claude` (native, or node with its process title set)
/// or as node/bun running `@anthropic-ai/claude-code`; opencode as
/// `opencode`.
pub fn agent_of(name: &str, args: &[String]) -> Option<&'static str> {
    let base = |s: &str| {
        let file = s.rsplit(['/', '\\']).next().unwrap_or(s).to_lowercase();
        file.strip_suffix(".exe")
            .map(str::to_string)
            .unwrap_or(file)
    };
    let mut names = std::iter::once(base(name)).chain(args.first().map(|a| base(a)));
    if let Some(agent) = names.find_map(|n| match n.as_str() {
        "claude" => Some("claude-code"),
        "opencode" => Some("opencode"),
        _ => None,
    }) {
        return Some(agent);
    }
    if matches!(base(name).as_str(), "node" | "bun" | "deno") {
        // The script is the first argument that isn't an option.
        let script = args.iter().skip(1).find(|a| !a.starts_with('-'))?;
        if script.contains("claude-code") || base(script) == "claude" {
            return Some("claude-code");
        }
        if script.contains("opencode") {
            return Some("opencode");
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn exe_name(name: &str) -> String {
    let name = name.to_lowercase();
    name.strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(name)
}

#[cfg(target_os = "linux")]
fn list() -> Vec<Process> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(|entry| {
        let entry = entry.ok()?;
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
        // `pid (comm) state ppid …`; comm may contain spaces and parens.
        let (comm, rest) = stat.split_once(" (")?.1.rsplit_once(") ")?;
        let ppid = rest.split(' ').nth(1)?.parse().ok()?;
        // Command lines are only needed to recognise scripts run by a
        // JavaScript runtime; skip everyone else's.
        let args: Vec<String> = if matches!(comm, "node" | "bun" | "deno") {
            std::fs::read(entry.path().join("cmdline"))
                .map(|raw| {
                    raw.split(|b| *b == 0)
                        .filter(|a| !a.is_empty())
                        .map(|a| String::from_utf8_lossy(a).into_owned())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // `comm` is cut to 15 bytes; prefer the executable's file name.
        let name = std::fs::read_link(entry.path().join("exe"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| comm.to_string());
        let agent = agent_of(comm, &args).or_else(|| agent_of(&name, &args));
        let cwd = agent.and_then(|_| std::fs::read_link(entry.path().join("cwd")).ok());
        Some(Process {
            pid,
            ppid,
            name: exe_name(&name),
            agent,
            cwd,
        })
    })
    .collect()
}

#[cfg(target_os = "macos")]
fn list() -> Vec<Process> {
    super::macos::processes()
}

#[cfg(windows)]
fn list() -> Vec<Process> {
    super::windows::processes()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn list() -> Vec<Process> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str, agent: Option<&'static str>) -> Process {
        Process {
            pid,
            ppid,
            name: name.to_string(),
            agent,
            cwd: agent.map(|_| PathBuf::from("/nonexistent/project")),
        }
    }

    fn table() -> Processes {
        Processes::from_list(vec![
            p(1, 0, "systemd", None),
            p(100, 1, "kitty", None),
            p(101, 100, "zsh", None),
            p(102, 101, "claude", Some("claude-code")),
            p(200, 1, "kitty", None),
            p(201, 200, "zsh", None),
            p(300, 1, "code", None),
        ])
    }

    fn session(agent: &str) -> AgentSession<'_> {
        AgentSession {
            agent,
            project: Path::new("/nonexistent/project"),
        }
    }

    fn app(app_id: &str, pid: Option<u32>) -> FocusedApp {
        FocusedApp {
            app_id: app_id.to_string(),
            pid,
        }
    }

    #[test]
    fn matches_the_window_that_runs_the_agent() {
        let t = table();
        assert!(t.window_runs_agent(&app("kitty", Some(100)), session("claude-code")));
        // Another window of the same terminal.
        assert!(!t.window_runs_agent(&app("kitty", Some(200)), session("claude-code")));
        assert!(!t.window_runs_agent(&app("code", Some(300)), session("claude-code")));
        assert!(!t.window_runs_agent(&app("kitty", Some(100)), session("opencode")));
    }

    #[test]
    fn without_a_pid_matches_by_app_id() {
        let t = table();
        assert!(t.window_runs_agent(&app("Kitty", None), session("claude-code")));
        assert!(t.window_runs_agent(&app("net.kovidgoyal.kitty", None), session("claude-code")));
        assert!(!t.window_runs_agent(&app("code", None), session("claude-code")));
    }

    #[test]
    fn other_projects_dont_match() {
        let t = table();
        let other = AgentSession {
            agent: "claude-code",
            project: Path::new("/nonexistent/other"),
        };
        assert!(!t.window_runs_agent(&app("kitty", Some(100)), other));
    }

    #[test]
    fn recognises_agents() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(agent_of("claude", &s(&["claude"])), Some("claude-code"));
        assert_eq!(agent_of("claude.exe", &[]), Some("claude-code"));
        assert_eq!(
            agent_of(
                "node",
                &s(&[
                    "node",
                    "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"
                ])
            ),
            Some("claude-code")
        );
        assert_eq!(agent_of("opencode", &s(&["opencode"])), Some("opencode"));
        assert_eq!(agent_of("node", &s(&["node", "server.js"])), None);
        assert_eq!(agent_of("zsh", &s(&["zsh"])), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_our_own_process() {
        let t = Processes::snapshot();
        let me = t.get(std::process::id()).expect("own process listed");
        assert!(t.ancestors(me.pid).count() >= 1);
    }
}
