//! A snapshot of the process table, used to tell whether the focused window
//! runs an agent: the agent's process is a descendant of the window's
//! process (a terminal, or an editor's built-in terminal).
//!
//! Only process ids, parent ids, executable names, agent command lines (to
//! recognise the agent), the agents' working directories and Claude Code's
//! session files (`sessions/<pid>.json`, which name the session a process
//! runs) are read, plus the executable paths of the processes in the
//! focused window (to see whether the user runs a program of the project).
//! Only those programs' paths relative to the project folder may leave the
//! machine.

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
    /// The agent session this process runs, where the agent says so
    /// (Claude Code's `sessions/<pid>.json`).
    pub session: Option<String>,
    /// The session runs in the background, shown through the agent's
    /// session manager instead of its own terminal.
    pub background: bool,
    /// This is the agent's session manager (`claude agents`), which shows
    /// background sessions.
    pub manager: Option<&'static str>,
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
        let mut list = list();
        for process in &mut list {
            if process.agent == Some("claude-code")
                && let Some((id, background)) =
                    crate::agents::claude_code::AgentIntegrationClaudeCode::session_of_pid(
                        process.pid,
                    )
            {
                process.session = Some(id);
                process.background = background;
            }
        }
        Self::from_list(list)
    }

    pub fn from_list(list: Vec<Process>) -> Self {
        Self {
            by_pid: list.into_iter().map(|p| (p.pid, p)).collect(),
        }
    }

    pub fn get(&self, pid: u32) -> Option<&Process> {
        self.by_pid.get(&pid)
    }

    /// Processes running `session`. A process whose session is known must
    /// run exactly that one; otherwise its working directory must be the
    /// session's project folder, and where that's unknown too, any process
    /// of that agent counts.
    pub fn agent_processes<'a>(
        &'a self,
        session: AgentSession<'a>,
    ) -> impl Iterator<Item = &'a Process> + 'a {
        let project = canonical(session.project);
        self.by_pid.values().filter(move |p| {
            p.agent == Some(session.agent)
                && match &p.session {
                    Some(id) => id == session.id,
                    None => p.cwd.as_deref().is_none_or(|cwd| canonical(cwd) == project),
                }
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
        self.agent_processes(session)
            .any(|agent| self.window_runs(app, agent.pid))
    }

    /// Whether the focused window runs `agent`'s session manager.
    pub fn window_runs_manager(&self, app: &FocusedApp, agent: &str) -> bool {
        self.by_pid
            .values()
            .filter(|p| p.manager == Some(agent))
            .any(|p| self.window_runs(app, p.pid))
    }

    /// The sessions of `agent` running in the background right now.
    pub fn background_sessions<'a>(&'a self, agent: &'a str) -> impl Iterator<Item = &'a str> {
        self.by_pid
            .values()
            .filter(move |p| p.agent == Some(agent) && p.background)
            .filter_map(|p| p.session.as_deref())
    }

    /// The agent processes running in the focused window: agent, working
    /// directory (canonical, where known) and session (where the agent
    /// names it).
    pub fn agents_in_window(&self, app: &FocusedApp) -> Vec<WindowAgent> {
        self.by_pid
            .values()
            .filter(|p| p.agent.is_some() && self.window_runs(app, p.pid))
            .map(|p| WindowAgent {
                agent: p.agent.unwrap_or_default(),
                cwd: p.cwd.as_deref().map(canonical),
                session: p.session.clone(),
            })
            .collect()
    }

    /// The executables (canonical paths) of the processes running in the
    /// focused window that no agent started: the window's own process and
    /// its descendants, stopping at agent processes and their children
    /// (an agent running the tests isn't the user testing). Without the
    /// window's pid: processes whose executable matches the app id, and
    /// their descendants.
    pub fn programs_in_window(&self, app: &FocusedApp) -> Vec<PathBuf> {
        let id = app_name(&app.app_id);
        let mut programs: Vec<PathBuf> = self
            .by_pid
            .values()
            .filter(|p| {
                for a in self.ancestors(p.pid) {
                    if a.agent.is_some() || a.manager.is_some() {
                        return false;
                    }
                    let window = match app.pid {
                        Some(window) => a.pid == window,
                        None => !id.is_empty() && a.name == id,
                    };
                    if window {
                        return true;
                    }
                }
                false
            })
            .filter_map(|p| exe_path(p.pid))
            .collect();
        programs.sort();
        programs.dedup();
        programs
    }

    /// The executables (canonical paths) of all processes no agent started,
    /// except this one. Paths are read for every process, so call this
    /// rarely (the tracker: every 10 s).
    pub fn programs_running(&self) -> Vec<PathBuf> {
        let me = std::process::id();
        let mut programs: Vec<PathBuf> = self
            .by_pid
            .values()
            .filter(|p| p.pid != me)
            .filter(|p| {
                !self
                    .ancestors(p.pid)
                    .any(|a| a.agent.is_some() || a.manager.is_some())
            })
            .filter_map(|p| exe_path(p.pid))
            .collect();
        programs.sort();
        programs.dedup();
        programs
    }

    /// Whether process `pid` descends from the focused window's process.
    /// Without the window's pid (wlr-foreign-toplevel gives only the app
    /// id): an ancestor's executable matches the app id, which can't tell
    /// two windows of the same terminal apart.
    fn window_runs(&self, app: &FocusedApp, pid: u32) -> bool {
        let mut ancestors = self.ancestors(pid).skip(1);
        match app.pid {
            Some(window) => ancestors.any(|p| p.pid == window),
            None => {
                let id = app_name(&app.app_id);
                !id.is_empty() && ancestors.any(|p| p.name == id)
            }
        }
    }
}

/// The last part of an app id, lowercased: `org.wezfurlong.wezterm` →
/// `wezterm`, `Alacritty` → `alacritty`.
fn app_name(app_id: &str) -> String {
    app_id.rsplit('.').next().unwrap_or(app_id).to_lowercase()
}

/// An agent process running in the focused window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowAgent {
    pub agent: &'static str,
    pub cwd: Option<PathBuf>,
    pub session: Option<String>,
}

impl WindowAgent {
    /// Whether this process runs `session`: exactly where the agent names
    /// its session, otherwise by working directory.
    pub fn runs(&self, session: AgentSession<'_>) -> bool {
        self.agent == session.agent
            && match &self.session {
                Some(id) => id == session.id,
                None => self
                    .cwd
                    .as_deref()
                    .is_none_or(|cwd| cwd == canonical(session.project)),
            }
    }
}

/// The project folder (canonical) if it is a git repository: only then do
/// its programs count as `testing`.
pub fn git_root(project: &Path) -> Option<PathBuf> {
    project.join(".git").exists().then(|| canonical(project))
}

/// The `programs` inside `root` (from [`git_root`]), as paths relative to
/// it with `/` separators.
pub fn programs_under(programs: &[PathBuf], root: &Path) -> Vec<String> {
    programs
        .iter()
        .filter_map(|p| p.strip_prefix(root).ok())
        .filter(|rel| !rel.as_os_str().is_empty())
        .map(|rel| {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

/// A process's executable, canonical where the OS gives a resolved path.
/// `None` if we may not read it.
#[cfg(target_os = "linux")]
pub fn exe_path(pid: u32) -> Option<PathBuf> {
    let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    // A binary replaced while running (a rebuild) reads as `… (deleted)`.
    let text = path.to_string_lossy();
    Some(match text.strip_suffix(" (deleted)") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    })
}

#[cfg(target_os = "macos")]
pub fn exe_path(pid: u32) -> Option<PathBuf> {
    super::macos::exe_path(i32::try_from(pid).ok()?).map(|p| canonical(&p))
}

#[cfg(windows)]
pub fn exe_path(pid: u32) -> Option<PathBuf> {
    super::windows::exe_path(pid).map(|p| canonical(Path::new(&p)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn exe_path(_pid: u32) -> Option<PathBuf> {
    None
}

/// `path` with symlinks resolved, or as it is if that fails.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Subcommands that run something other than a session: the session
/// manager, the background daemon, setup and maintenance.
const NOT_A_SESSION: &[&str] = &[
    "agents",
    "auth",
    "config",
    "daemon",
    "doctor",
    "install",
    "mcp",
    "migrate-installer",
    "plugin",
    "plugins",
    "ps",
    "remote-control",
    "setup-token",
    "update",
    "upgrade",
    // opencode
    "acp",
    "export",
    "github",
    "import",
    "models",
    "serve",
    "stats",
    "web",
];

/// The agent session a process runs, from its executable name and command
/// line. Claude Code runs as `claude` (native, or node with its process
/// title set) or as node/bun running `@anthropic-ai/claude-code`; opencode
/// as `opencode`. Their other subcommands (`claude agents`, `claude daemon`,
/// `opencode serve`, …) don't count.
pub fn agent_of(name: &str, args: &[String]) -> Option<&'static str> {
    let agent = runtime_agent(name, args)?;
    match subcommand(name, args) {
        Some(sub) if NOT_A_SESSION.contains(&sub.as_str()) => None,
        _ => Some(agent),
    }
}

/// Whether `path` is a Claude Code build from its native installer, which
/// keeps each version as `…/claude/versions/<version>` and runs it under
/// that name (with its process title set to `claude …`).
pub fn is_claude_build(path: &Path) -> bool {
    let dir = path.parent();
    dir.and_then(Path::file_name)
        .is_some_and(|n| n == "versions")
        && dir
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|n| n == "claude")
}

/// Whether an executable name looks like a version number (`2.1.330`), as
/// Claude Code's native builds are named.
pub fn is_version_name(name: &str) -> bool {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    name.contains('.') && name.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The agent whose session manager a process runs (`claude agents`).
pub fn manager_of(name: &str, args: &[String]) -> Option<&'static str> {
    let agent = runtime_agent(name, args)?;
    (agent == "claude-code" && subcommand(name, args).is_some_and(|s| s == "agents"))
        .then_some(agent)
}

/// The first argument after the program (or the script run by node, bun
/// or deno), if any.
fn subcommand<'a>(name: &str, args: &'a [String]) -> Option<&'a String> {
    let runs_script = matches!(base_name(name).as_str(), "node" | "bun" | "deno");
    if runs_script {
        let script = args.iter().skip(1).position(|a| !a.starts_with('-'))? + 1;
        args.get(script + 1)
    } else {
        args.get(1)
    }
}

fn base_name(s: &str) -> String {
    let file = s.rsplit(['/', '\\']).next().unwrap_or(s).to_lowercase();
    file.strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(file)
}

fn runtime_agent(name: &str, args: &[String]) -> Option<&'static str> {
    let base = base_name;
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
        // `comm` is cut to 15 bytes; prefer the executable's file name.
        let exe = std::fs::read_link(entry.path().join("exe")).ok();
        let mut name = exe
            .as_deref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| comm.to_string());
        if exe.as_deref().is_some_and(is_claude_build) {
            name = "claude".to_string();
        }
        // Command lines are only needed to recognise agents and scripts run
        // by a JavaScript runtime; skip everyone else's.
        let args: Vec<String> =
            if matches!(
                name.as_str(),
                "node" | "bun" | "deno" | "claude" | "opencode"
            ) || matches!(comm, "node" | "bun" | "deno" | "claude" | "opencode")
            {
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
        let agent = agent_of(comm, &args).or_else(|| agent_of(&name, &args));
        let manager = manager_of(comm, &args).or_else(|| manager_of(&name, &args));
        let cwd = agent.and_then(|_| std::fs::read_link(entry.path().join("cwd")).ok());
        Some(Process {
            pid,
            ppid,
            name: exe_name(&name),
            agent,
            cwd,
            session: None,
            background: false,
            manager,
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
            session: None,
            background: false,
            manager: None,
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
            id: "s1",
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
            id: "s1",
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
        assert_eq!(
            agent_of("claude", &s(&["claude", "--resume"])),
            Some("claude-code")
        );
        assert_eq!(
            agent_of("claude", &s(&["claude", "fix the tests"])),
            Some("claude-code")
        );
        assert_eq!(agent_of("claude", &s(&["claude", "agents"])), None);
        assert_eq!(
            agent_of("claude", &s(&["/x/claude", "daemon", "run"])),
            None
        );
        assert_eq!(agent_of("opencode", &s(&["opencode", "serve"])), None);
        assert_eq!(
            agent_of(
                "node",
                &s(&["node", "/x/claude-code/cli.js", "mcp", "serve"])
            ),
            None
        );
    }

    #[test]
    fn recognises_native_claude_builds() {
        assert!(is_claude_build(Path::new(
            "/home/u/.local/share/claude/versions/2.1.330"
        )));
        assert!(!is_claude_build(Path::new("/usr/lib/versions/2.1.330")));
        assert!(is_version_name("2.1.330"));
        assert!(is_version_name("2.1.330.exe"));
        assert!(!is_version_name("claude"));
        assert!(!is_version_name("12"));
    }

    #[test]
    fn recognises_the_session_manager() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(
            manager_of("claude", &s(&["claude", "agents"])),
            Some("claude-code")
        );
        assert_eq!(manager_of("claude", &s(&["claude"])), None);
        assert_eq!(manager_of("claude", &s(&["claude", "daemon", "run"])), None);

        let mut list = table().by_pid.into_values().collect::<Vec<_>>();
        list.push(Process {
            manager: Some("claude-code"),
            ..p(203, 201, "claude", None)
        });
        list.push(Process {
            session: Some("bg1".into()),
            background: true,
            ..p(400, 1, "claude", Some("claude-code"))
        });
        let t = Processes::from_list(list);
        assert!(t.window_runs_manager(&app("kitty", Some(200)), "claude-code"));
        assert!(!t.window_runs_manager(&app("kitty", Some(100)), "claude-code"));
        assert_eq!(
            t.background_sessions("claude-code").collect::<Vec<_>>(),
            ["bg1"]
        );
    }

    #[test]
    fn a_known_session_matches_only_itself() {
        let mut list = table().by_pid.into_values().collect::<Vec<_>>();
        for p in &mut list {
            if p.pid == 102 {
                p.session = Some("s2".into());
            }
        }
        let t = Processes::from_list(list);
        // Same folder, but the process runs another session.
        assert!(!t.window_runs_agent(&app("kitty", Some(100)), session("claude-code")));
        let s2 = AgentSession {
            id: "s2",
            ..session("claude-code")
        };
        assert!(t.window_runs_agent(&app("kitty", Some(100)), s2));
    }

    #[test]
    fn programs_inside_the_project_are_relative() {
        let root = Path::new("/nonexistent/project");
        let programs = [
            PathBuf::from("/nonexistent/project/target/debug/game"),
            PathBuf::from("/usr/bin/zsh"),
            PathBuf::from("/nonexistent/project-other/bin/x"),
        ];
        assert_eq!(programs_under(&programs, root), ["target/debug/game"]);
    }

    #[test]
    fn a_folder_without_git_has_no_programs() {
        let dir = std::env::temp_dir().join(format!("sisyphus-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git_root(&dir), None);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        assert_eq!(git_root(&dir), Some(canonical(&dir)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn programs_in_the_window_skip_what_agents_started() {
        // The test process plays the window; its executable is ours.
        let me = std::process::id();
        // A made-up pid plays the window; the test process runs in it.
        let window = 4_000_000_000;
        let t = Processes::from_list(vec![
            p(window, 0, "kitty", None),
            p(me, window, "game", None),
        ]);
        let exe = std::env::current_exe().unwrap();
        assert_eq!(t.programs_in_window(&app("kitty", Some(window))), [exe]);
        // The same process started by an agent doesn't count.
        let t = Processes::from_list(vec![
            p(window, 0, "kitty", None),
            p(50, window, "claude", Some("claude-code")),
            p(me, 50, "game", None),
        ]);
        assert!(t.programs_in_window(&app("kitty", Some(window))).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_our_own_process() {
        let t = Processes::snapshot();
        let me = t.get(std::process::id()).expect("own process listed");
        assert!(t.ancestors(me.pid).count() >= 1);
    }
}
