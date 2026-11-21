//! Linux: X11, or Wayland with a per-compositor focus source.
//!
//! Wayland has no standard way to see another app's window, so the focused
//! app comes from the first of these that works:
//!
//! 1. Hyprland: its IPC socket (`activewindow`), with the window's pid.
//! 2. Sway: its IPC socket (`get_tree`), with the window's pid.
//! 3. GNOME: the "Window Calls" Shell extension over D-Bus, if the user has
//!    it installed. GNOME itself doesn't expose the focused window.
//! 4. KDE Plasma: a small KWin script that reports the active window's
//!    class and pid to us over D-Bus.
//! 5. Any compositor with `wlr-foreign-toplevel-management` (niri, river,
//!    labwc, Wayfire, …): the activated toplevel's app id, without a pid.
//!
//! Idle time comes from `ext-idle-notify`, or Mutter's IdleMonitor on GNOME.

use std::time::Duration;

use super::FocusReading;

mod gnome;
mod hyprland;
mod kwin;
mod sway;
mod wayland;
mod x11;

/// Whether this is a Wayland session.
pub fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
}

fn desktop_is(name: &str) -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|d| d.split(':').any(|part| part.eq_ignore_ascii_case(name)))
}

trait IdleSource: Send {
    fn since_last_input(&mut self) -> Option<Duration>;
    fn name(&self) -> &'static str;
}

trait FocusSource: Send {
    fn focused(&mut self) -> FocusReading;
    fn name(&self) -> &'static str;
}

pub struct Monitor {
    idle: Option<Box<dyn IdleSource>>,
    focus: Option<Box<dyn FocusSource>>,
}

impl Monitor {
    pub fn start() -> Self {
        if is_wayland() {
            Self::wayland()
        } else {
            Self {
                idle: x11::Idle::connect().map(|s| Box::new(s) as _),
                focus: x11::Focus::connect().map(|s| Box::new(s) as _),
            }
        }
    }

    fn wayland() -> Self {
        let mut focus: Option<Box<dyn FocusSource>> = None;
        if let Some(source) = hyprland::Hyprland::detect() {
            focus = Some(Box::new(source));
        } else if let Some(source) = sway::Sway::detect() {
            focus = Some(Box::new(source));
        } else if desktop_is("GNOME") {
            focus = gnome::Focus::connect().map(|s| Box::new(s) as _);
        } else if desktop_is("KDE") {
            focus = kwin::Kwin::start().map(|s| Box::new(s) as _);
        }
        let want_toplevels = focus.is_none();
        let session = wayland::Session::connect(want_toplevels);

        let mut idle: Option<Box<dyn IdleSource>> = None;
        if let Some(session) = &session {
            if session.has_idle() {
                idle = Some(Box::new(session.idle()));
            }
            if want_toplevels && session.has_toplevels() {
                focus = Some(Box::new(session.toplevels()));
            }
        }
        if idle.is_none() && desktop_is("GNOME") {
            idle = gnome::Idle::connect().map(|s| Box::new(s) as _);
        }
        Self { idle, focus }
    }

    pub fn since_last_input(&mut self) -> Option<Duration> {
        self.idle.as_mut()?.since_last_input()
    }

    pub fn focused(&mut self) -> FocusReading {
        match &mut self.focus {
            Some(source) => source.focused(),
            None => FocusReading::Unknown,
        }
    }

    pub fn describe(&self) -> String {
        let idle = self.idle.as_ref().map_or("none", |s| s.name());
        let focus = self.focus.as_ref().map_or("none", |s| s.name());
        format!("input: {idle}, focus: {focus}")
    }
}
