//! Wayland protocols, on one connection and one background thread:
//!
//! - `ext-idle-notify-v1`: tells us when the user has been idle for a
//!   second and when they're back. Version 2's input notification ignores
//!   idle inhibitors, so a playing video doesn't count as input.
//! - `wlr-foreign-toplevel-management-v1`: every
//!   toplevel's app id and whether it's activated. Titles arrive too but are
//!   dropped on the spot.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_callback, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::{FocusSource, IdleSource};
use crate::activity::{FocusReading, FocusedApp};

/// Idle after this long without input. Input is therefore seen with a
/// one-second resolution, plenty for 10-second slots.
const IDLE_AFTER: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Shared {
    /// Since when the user is idle; `None` while active.
    idle_since: Option<Instant>,
    toplevels: HashMap<ObjectId, Toplevel>,
}

#[derive(Default)]
struct Toplevel {
    app_id: String,
    activated: bool,
    /// Changes arrive in batches ending with `done`.
    pending_app_id: Option<String>,
    pending_activated: Option<bool>,
}

struct State {
    shared: Arc<Mutex<Shared>>,
}

/// The connection and its thread. The thread stops when the last handle
/// is dropped.
struct Inner {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    conn: Connection,
    qh: QueueHandle<State>,
    has_idle: bool,
    has_toplevels: bool,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the thread's blocking dispatch with a round trip.
        self.conn.display().sync(&self.qh, ());
        let _ = self.conn.flush();
    }
}

#[derive(Clone)]
pub struct Session(Arc<Inner>);

impl Session {
    /// Connect and bind what the compositor offers. `toplevels`: also track
    /// windows (when no compositor IPC tells us the focus).
    pub fn connect(toplevels: bool) -> Option<Self> {
        let conn = Connection::connect_to_env().ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut state = State {
            shared: shared.clone(),
        };

        let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=1, ()).ok();
        let notifier = globals.bind::<ExtIdleNotifierV1, _, _>(&qh, 1..=2, ()).ok();
        let has_idle = match (&notifier, &seat) {
            (Some(notifier), Some(seat)) => {
                let ms = IDLE_AFTER.as_millis() as u32;
                if notifier.version() >= 2 {
                    notifier.get_input_idle_notification(ms, seat, &qh, ());
                } else {
                    notifier.get_idle_notification(ms, seat, &qh, ());
                }
                true
            }
            _ => false,
        };
        let has_toplevels = toplevels
            && globals
                .bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ())
                .is_ok();
        if !has_idle && !has_toplevels {
            return None;
        }
        // Receive the current windows before the first reading.
        queue.roundtrip(&mut state).ok()?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("wayland-activity".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    if queue.blocking_dispatch(&mut state).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self(Arc::new(Inner {
            shared,
            stop,
            conn,
            qh,
            has_idle,
            has_toplevels,
        })))
    }

    pub fn has_idle(&self) -> bool {
        self.0.has_idle
    }

    pub fn has_toplevels(&self) -> bool {
        self.0.has_toplevels
    }

    pub fn idle(&self) -> Idle {
        Idle(self.clone())
    }

    pub fn toplevels(&self) -> Toplevels {
        Toplevels(self.clone())
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.0.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Idle(Session);

impl IdleSource for Idle {
    fn since_last_input(&mut self) -> Option<Duration> {
        Some(match self.0.shared().idle_since {
            Some(since) => since.elapsed(),
            // Active: input within the last second.
            None => Duration::ZERO,
        })
    }

    fn name(&self) -> &'static str {
        "ext-idle-notify"
    }
}

pub struct Toplevels(Session);

impl FocusSource for Toplevels {
    fn focused(&mut self) -> FocusReading {
        let shared = self.0.shared();
        match shared.toplevels.values().find(|t| t.activated) {
            Some(t) => FocusReading::App(FocusedApp {
                app_id: t.app_id.clone(),
                pid: None,
            }),
            None => FocusReading::Nothing,
        }
    }

    fn name(&self) -> &'static str {
        "wlr-foreign-toplevel"
    }
}

impl State {
    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut shared = state.shared();
        match event {
            ext_idle_notification_v1::Event::Idled => {
                shared.idle_since = Some(Instant::now() - IDLE_AFTER);
            }
            ext_idle_notification_v1::Event::Resumed => shared.idle_since = None,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state
                .shared()
                .toplevels
                .insert(toplevel.id(), Toplevel::default());
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

/// `activated` in the toplevel state array.
const ACTIVATED: u32 = 2;

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let mut shared = state.shared();
        if let Event::Closed = event {
            shared.toplevels.remove(&handle.id());
            drop(shared);
            handle.destroy();
            return;
        }
        let Some(toplevel) = shared.toplevels.get_mut(&handle.id()) else {
            return;
        };
        match event {
            Event::AppId { app_id } => toplevel.pending_app_id = Some(app_id),
            Event::State { state } => {
                let activated = state
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .any(|s| s == ACTIVATED);
                toplevel.pending_activated = Some(activated);
            }
            Event::Done => {
                if let Some(app_id) = toplevel.pending_app_id.take() {
                    toplevel.app_id = app_id;
                }
                if let Some(activated) = toplevel.pending_activated.take() {
                    toplevel.activated = activated;
                }
            }
            // Titles, outputs and parents aren't needed; titles are never
            // kept.
            _ => {}
        }
    }
}
