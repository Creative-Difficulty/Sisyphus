//! What the user is doing right now, read from the OS without a keylogger
//! and without root:
//!
//! - **time since the last input** (any key or mouse event; which key is
//!   never seen), from the OS idle-time APIs
//! - **the focused app**, reduced to a coarse [`Focus`] class
//!
//! | OS            | last input                        | focused app                         |
//! |---------------|-----------------------------------|-------------------------------------|
//! | Windows       | `GetLastInputInfo`                | `GetForegroundWindow` → process     |
//! | macOS         | `CGEventSourceSecondsSinceLastEventType` | `NSWorkspace.frontmostApplication` |
//! | Linux X11     | XScreenSaver                      | `_NET_ACTIVE_WINDOW`, `WM_CLASS`, `_NET_WM_PID` |
//! | Linux Wayland | `ext-idle-notify` (GNOME: Mutter's IdleMonitor) | see [`linux`]          |
//!
//! Nothing is read without the user's consent, on every OS (asked during
//! onboarding, saved as `Config::activity_allowed`).
//!
//! The app id and process id of a [`FocusedApp`] stay on this machine: only
//! the [`Focus`] class may go into a heartbeat. Window titles are never read
//! into this module's types.

use std::path::Path;
use std::time::Duration;

mod classify;
pub mod process;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use classify::{AppClass, classify};
pub use process::Processes;

/// The focus class of one heartbeat slot, as in the heartbeat scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The window the session's agent runs in.
    Agent,
    Terminal,
    Editor,
    Browser,
    Other,
    /// A program of the session's project: an executable inside its folder,
    /// which is a git repository, and not started by an agent.
    Testing,
    /// Nothing has focus (for example the desktop or a lock screen).
    None,
}

impl Focus {
    /// The name used in heartbeats.
    pub fn as_str(self) -> &'static str {
        match self {
            Focus::Agent => "agent",
            Focus::Terminal => "terminal",
            Focus::Editor => "editor",
            Focus::Browser => "browser",
            Focus::Other => "other",
            Focus::Testing => "testing",
            Focus::None => "none",
        }
    }
}

/// The app in front. Local only: never upload `app_id` or `pid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedApp {
    /// The app's id as the OS names it: a Wayland app id, an X11 `WM_CLASS`
    /// class, a macOS bundle id or a Windows executable name.
    pub app_id: String,
    /// The process that owns the window, where the OS tells us.
    pub pid: Option<u32>,
}

/// One reading of the focused app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusReading {
    /// This system can't tell us (or reading it isn't allowed).
    Unknown,
    /// Nothing has focus.
    Nothing,
    App(FocusedApp),
}

/// An agent session to match the focused window against.
#[derive(Debug, Clone, Copy)]
pub struct AgentSession<'a> {
    /// [`crate::agents::Integration::id`].
    pub agent: &'a str,
    /// The session's project folder.
    pub project: &'a Path,
    /// The agent's session id.
    pub id: &'a str,
}

impl FocusReading {
    /// The focus class for `session`: [`Focus::Agent`] if the focused window
    /// runs that session's agent, [`Focus::Testing`] if it runs a program of
    /// the session's project. `None` when the focus is unknown.
    pub fn focus(&self, session: Option<AgentSession<'_>>, processes: &Processes) -> Option<Focus> {
        match self {
            FocusReading::Unknown => None,
            FocusReading::Nothing => Some(Focus::None),
            FocusReading::App(app) => {
                if let Some(session) = session
                    && processes.window_runs_agent(app, session)
                {
                    return Some(Focus::Agent);
                }
                if let Some(session) = session
                    && let Some(root) = process::git_root(session.project)
                    && !process::programs_under(&processes.programs_in_window(app), &root)
                        .is_empty()
                {
                    return Some(Focus::Testing);
                }
                Some(match classify(&app.app_id) {
                    AppClass::Terminal => Focus::Terminal,
                    AppClass::Editor => Focus::Editor,
                    AppClass::Browser => Focus::Browser,
                    AppClass::Other => Focus::Other,
                })
            }
        }
    }
}

/// Reads input idle time and the focused app. Create one and keep it: some
/// backends keep a connection or a background thread open.
pub struct Monitor {
    /// `None` without the user's consent.
    inner: Option<Inner>,
}

#[cfg(all(unix, not(target_os = "macos")))]
type Inner = linux::Monitor;
#[cfg(target_os = "macos")]
type Inner = macos::Monitor;
#[cfg(windows)]
type Inner = windows::Monitor;

impl Monitor {
    /// `allowed`: the user agreed to activity being read. Without it,
    /// nothing is read and every reading is unknown.
    pub fn new(allowed: bool) -> Self {
        Self {
            inner: allowed.then(Inner::start),
        }
    }

    /// Time since the last keyboard or mouse input anywhere on the system.
    /// `None` if this system can't tell.
    pub fn since_last_input(&mut self) -> Option<Duration> {
        self.inner.as_mut()?.since_last_input()
    }

    /// The app in front right now.
    pub fn focused(&mut self) -> FocusReading {
        match &mut self.inner {
            Some(inner) => inner.focused(),
            None => FocusReading::Unknown,
        }
    }

    /// Which backends are in use, for the `activity` command.
    pub fn describe(&self) -> String {
        match &self.inner {
            Some(inner) => inner.describe(),
            None => "not allowed".to_string(),
        }
    }
}
