//! The tray icon: keeps Sisyphus reachable while its window is closed.
//! Hovering shows a little info, the menu shows the same info plus "Open
//! Sisyphus" and "Quit", and a left click opens the window (on macOS any
//! click opens the menu).
//!
//! Linux uses StatusNotifierItem over D-Bus (KDE, waybar, XFCE, GNOME with
//! the AppIndicator extension; plain GNOME has no tray), Windows and macOS
//! their native tray.

use std::sync::mpsc::{Receiver, channel};

#[cfg(all(unix, not(target_os = "macos")))]
mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
use linux as backend;
#[cfg(any(windows, target_os = "macos"))]
mod native;
#[cfg(any(windows, target_os = "macos"))]
use native as backend;

/// What the user did with the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Open (or bring forward) the window.
    Open,
    /// Quit Sisyphus.
    Quit,
}

/// What the tray shows: one line each, in the menu (greyed out) and the
/// tooltip.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub lines: Vec<String>,
}

impl Info {
    /// The tooltip: the name, then the lines. (Linux sends the name and
    /// the lines separately.)
    #[cfg_attr(all(unix, not(target_os = "macos")), allow(dead_code))]
    fn tooltip(&self) -> String {
        std::iter::once("Sisyphus")
            .chain(self.lines.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The tray icon. Removed when dropped.
pub struct Tray {
    inner: backend::Tray,
}

impl Tray {
    /// Show the tray icon. Fails where there is no tray (plain GNOME, or
    /// an unsupported system); the window must then quit when closed.
    /// Clicks and menu choices arrive on the returned receiver.
    ///
    /// On Windows and macOS this must run on the main thread, once the
    /// event loop is running.
    pub fn new() -> Result<(Self, Receiver<Event>), String> {
        let (tx, rx) = channel();
        let inner = backend::Tray::new(tx)?;
        Ok((Self { inner }, rx))
    }

    /// Show new info. Cheap to call with unchanged info.
    pub fn set_info(&mut self, info: Info) {
        self.inner.set_info(info);
    }
}

/// Shorten `text` to at most `max` characters, ending in "…" if cut.
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max.saturating_sub(1)).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_long_text() {
        assert_eq!(shorten("Sisyphus", 20), "Sisyphus");
        assert_eq!(shorten("Fix the login page now", 10), "Fix the l…");
        assert_eq!(shorten("Fix the login", 9), "Fix the…");
        assert_eq!(shorten("abcdef", 4), "abc…");
    }

    #[test]
    fn tooltip_starts_with_the_name() {
        let info = Info {
            lines: vec!["Today: 2h 14m".into(), "No agent working".into()],
        };
        assert_eq!(info.tooltip(), "Sisyphus\nToday: 2h 14m\nNo agent working");
    }
}
