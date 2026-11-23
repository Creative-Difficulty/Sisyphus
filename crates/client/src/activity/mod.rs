//! What the user is doing right now, read from the OS without a keylogger
//! and without root:
//!
//! - **time since the last input** (any key or mouse event; which key is
//!   never seen), from the OS idle-time APIs
//! - **how many key presses and mouse events** happened (never which
//!   keys): exact on macOS (CoreGraphics event counters) and on Linux
//!   Wayland with the input helper ([`crate::input_helper`]), inferred from
//!   the idle time and the cursor on Windows and X11 ([`sampler`])
//! - **the focused app**, reduced to a coarse [`Focus`] class
//!
//! | OS            | last input                        | focused app                         |
//! |---------------|-----------------------------------|-------------------------------------|
//! | Windows       | `GetLastInputInfo`                | `GetForegroundWindow` → process     |
//! | macOS         | `CGEventSourceSecondsSinceLastEventType` | `NSWorkspace.frontmostApplication` |
//! | Linux X11     | XScreenSaver                      | `_NET_ACTIVE_WINDOW`, `WM_CLASS`, `_NET_WM_PID` |
//! | Linux Wayland | `ext-idle-notify` (GNOME: Mutter's IdleMonitor) | see [`linux`]          |
//!
//! Activity is always read (decided 2026-11-21: no consent step).
//!
//! The app id and process id of a [`FocusedApp`] stay on this machine: only
//! the [`Focus`] class may go into a heartbeat. Window titles are never read
//! into this module's types.

use std::path::Path;
use std::time::Duration;

mod classify;
pub mod process;
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
mod sampler;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use classify::{AppClass, classify};
pub use process::Processes;
pub use sisyphus_common::heartbeat::InputKinds;

/// Key presses and mouse events (movements, clicks, scrolling) counted over
/// some time. Never which keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputCounts {
    pub keys: u32,
    pub mouse: u32,
    /// The part of `keys` and `mouse` that came from software rather than
    /// a physical device, where the system tells: events posted by apps
    /// (macOS), virtual uinput devices (Linux input helper). 0 elsewhere.
    pub injected_keys: u32,
    pub injected_mouse: u32,
}

impl InputCounts {
    pub fn add(&mut self, other: InputCounts) {
        self.keys = self.keys.saturating_add(other.keys);
        self.mouse = self.mouse.saturating_add(other.mouse);
        self.injected_keys = self.injected_keys.saturating_add(other.injected_keys);
        self.injected_mouse = self.injected_mouse.saturating_add(other.injected_mouse);
    }
}

/// Whether this system needs the input helper to tell keys from mouse
/// (Linux Wayland) and doesn't have it running.
pub fn input_helper_missing() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::is_wayland() && !linux::helper::available()
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        false
    }
}

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

    /// Key presses and mouse events since the last call, and how they were
    /// told apart. `None` when this system can't tell them apart right now.
    pub fn take_input(&mut self) -> Option<(InputCounts, InputKinds)> {
        self.inner.as_mut()?.take_input()
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
