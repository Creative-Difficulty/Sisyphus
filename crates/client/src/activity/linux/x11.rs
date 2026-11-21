//! X11: XScreenSaver for idle time, EWMH properties for the active window.

use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use super::{FocusSource, IdleSource};
use crate::activity::{FocusReading, FocusedApp};

pub struct Idle {
    conn: RustConnection,
    root: Window,
}

impl Idle {
    pub fn connect() -> Option<Self> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        // Check the extension is there once, up front.
        conn.screensaver_query_version(1, 1).ok()?.reply().ok()?;
        Some(Self { conn, root })
    }
}

impl IdleSource for Idle {
    fn since_last_input(&mut self) -> Option<Duration> {
        let info = self
            .conn
            .screensaver_query_info(self.root)
            .ok()?
            .reply()
            .ok()?;
        Some(Duration::from_millis(info.ms_since_user_input.into()))
    }

    fn name(&self) -> &'static str {
        "XScreenSaver"
    }
}

pub struct Focus {
    conn: RustConnection,
    root: Window,
    active: u32,
    pid: u32,
}

impl Focus {
    pub fn connect() -> Option<Self> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atom = |name: &[u8]| -> Option<u32> {
            Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
        };
        let active = atom(b"_NET_ACTIVE_WINDOW")?;
        let pid = atom(b"_NET_WM_PID")?;
        Some(Self {
            conn,
            root,
            active,
            pid,
        })
    }

    fn read(&self) -> Option<FocusReading> {
        let reply = self
            .conn
            .get_property(false, self.root, self.active, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        let window = match reply.value32().and_then(|mut v| v.next()) {
            Some(0) | None => return Some(FocusReading::Nothing),
            Some(window) => window,
        };
        // WM_CLASS is "instance\0class\0"; the class names the app.
        let class = self
            .conn
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
            .ok()?
            .reply()
            .ok()?;
        let mut parts = class.value.split(|b| *b == 0).filter(|p| !p.is_empty());
        let instance = parts.next();
        let app_id = parts
            .next()
            .or(instance)
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .unwrap_or_default();
        let pid = self
            .conn
            .get_property(false, window, self.pid, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()));
        Some(FocusReading::App(FocusedApp { app_id, pid }))
    }
}

impl FocusSource for Focus {
    fn focused(&mut self) -> FocusReading {
        self.read().unwrap_or(FocusReading::Unknown)
    }

    fn name(&self) -> &'static str {
        "X11"
    }
}
