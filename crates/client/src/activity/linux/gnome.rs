//! GNOME on Wayland, over the session D-Bus.
//!
//! - Idle time: Mutter's `org.gnome.Mutter.IdleMonitor`, built in.
//! - Focus: GNOME doesn't expose the focused window. The "Window Calls"
//!   Shell extension (extensions.gnome.org, by ickyicky) adds
//!   `org.gnome.Shell.Extensions.Windows.List`, which returns every window
//!   as JSON. Without the extension the focus stays unknown. Only
//!   `wm_class`, `pid` and `focus` are read from its reply; titles are
//!   dropped.

use std::time::Duration;

use serde_json::Value;
use zbus::blocking::Connection;

use super::{FocusSource, IdleSource};
use crate::activity::{FocusReading, FocusedApp};

pub struct Idle {
    conn: Connection,
}

impl Idle {
    pub fn connect() -> Option<Self> {
        let mut idle = Self {
            conn: Connection::session().ok()?,
        };
        idle.since_last_input().map(|_| idle)
    }
}

impl IdleSource for Idle {
    fn since_last_input(&mut self) -> Option<Duration> {
        let reply = self
            .conn
            .call_method(
                Some("org.gnome.Mutter.IdleMonitor"),
                "/org/gnome/Mutter/IdleMonitor/Core",
                Some("org.gnome.Mutter.IdleMonitor"),
                "GetIdletime",
                &(),
            )
            .ok()?;
        let ms: u64 = reply.body().deserialize().ok()?;
        Some(Duration::from_millis(ms))
    }

    fn name(&self) -> &'static str {
        "Mutter IdleMonitor"
    }
}

pub struct Focus {
    conn: Connection,
}

impl Focus {
    pub fn connect() -> Option<Self> {
        Some(Self {
            conn: Connection::session().ok()?,
        })
    }

    fn read(&self) -> Option<FocusReading> {
        let reply = self
            .conn
            .call_method(
                Some("org.gnome.Shell"),
                "/org/gnome/Shell/Extensions/Windows",
                Some("org.gnome.Shell.Extensions.Windows"),
                "List",
                &(),
            )
            .ok()?;
        let json: String = reply.body().deserialize().ok()?;
        parse(&json)
    }
}

fn parse(json: &str) -> Option<FocusReading> {
    let windows: Vec<Value> = serde_json::from_str(json).ok()?;
    let Some(window) = windows
        .iter()
        .find(|w| w.get("focus").and_then(Value::as_bool) == Some(true))
    else {
        return Some(FocusReading::Nothing);
    };
    let app_id = window
        .get("wm_class")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let pid = window
        .get("pid")
        .and_then(Value::as_i64)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0);
    Some(FocusReading::App(FocusedApp { app_id, pid }))
}

impl FocusSource for Focus {
    fn focused(&mut self) -> FocusReading {
        // Without the extension the call fails: focus unknown.
        self.read().unwrap_or(FocusReading::Unknown)
    }

    fn name(&self) -> &'static str {
        "GNOME Window Calls extension"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_window_list() {
        let json = r#"[
            {"wm_class":"firefox","pid":10,"focus":false,"title":"a"},
            {"wm_class":"org.gnome.Ptyxis","pid":11,"focus":true,"title":"secret"}
        ]"#;
        assert_eq!(
            parse(json),
            Some(FocusReading::App(FocusedApp {
                app_id: "org.gnome.Ptyxis".to_string(),
                pid: Some(11),
            }))
        );
        assert_eq!(parse("[]"), Some(FocusReading::Nothing));
    }
}
