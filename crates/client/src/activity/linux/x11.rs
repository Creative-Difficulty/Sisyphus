//! X11: XScreenSaver for idle time, EWMH properties for the active window,
//! and keys told from mouse by polling the idle time and the pointer
//! ([`crate::activity::sampler`]).

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, KeyButMask, Window};
use x11rb::rust_connection::RustConnection;

use super::{FocusSource, IdleSource};
use crate::activity::sampler::{Inferred, Probe};
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

/// Start counting keys and mouse events on their own connection.
pub fn input_kinds() -> Option<Inferred> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    conn.screensaver_query_version(1, 1).ok()?.reply().ok()?;
    let start = Instant::now();
    let buttons = KeyButMask::BUTTON1 | KeyButMask::BUTTON2 | KeyButMask::BUTTON3;
    // The idle time is in ms; with the round trips the computed input time
    // wobbles by a few ms.
    Inferred::start(15, move || {
        let idle = conn
            .screensaver_query_info(root)
            .ok()?
            .reply()
            .ok()?
            .ms_since_user_input;
        let pointer = conn.query_pointer(root).ok()?.reply().ok()?;
        let now = start.elapsed().as_millis() as u64;
        Some(Probe {
            last_input_ms: now.saturating_sub(idle.into()),
            cursor: (pointer.root_x.into(), pointer.root_y.into()),
            button_down: u16::from(pointer.mask) & u16::from(buttons) != 0,
        })
    })
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
