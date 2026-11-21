//! KDE Plasma on Wayland: KWin doesn't let ordinary apps see other windows,
//! but it runs scripts. We load a tiny script that calls us over D-Bus with
//! the active window's class and pid whenever it changes, and unload it
//! when we're done.
//!
//! The script only passes `resourceClass` and `pid`, never captions.

use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;

use super::FocusSource;
use crate::activity::{FocusReading, FocusedApp};

/// Our bus name and object, which the script calls (the script spells
/// them out too).
const BUS_NAME: &str = "dev.aelx.Sisyphus";
const OBJECT: &str = "/Focus";
/// The script's plugin name in KWin, used to unload it again.
const PLUGIN: &str = "sisyphus-focus";

/// Plasma 6 names the signal `windowActivated`, Plasma 5 `clientActivated`.
const SCRIPT: &str = r#"
function report(w) {
    if (w) {
        callDBus("dev.aelx.Sisyphus", "/Focus", "dev.aelx.Sisyphus.Focus",
            "Active", String(w.resourceClass || ""), Number(w.pid || 0));
    } else {
        callDBus("dev.aelx.Sisyphus", "/Focus", "dev.aelx.Sisyphus.Focus",
            "Active", "", 0);
    }
}
if (workspace.windowActivated) {
    workspace.windowActivated.connect(report);
    report(workspace.activeWindow);
} else {
    workspace.clientActivated.connect(report);
    report(workspace.activeClient);
}
"#;

type Latest = Arc<Mutex<FocusReading>>;

struct Sink(Latest);

#[zbus::interface(name = "dev.aelx.Sisyphus.Focus")]
impl Sink {
    /// Called by the KWin script. Only callers on this user's session bus
    /// can reach it.
    fn active(&self, class: String, pid: i32) {
        // A window without a class (no app id) is still a window when it
        // has a pid.
        let pid = u32::try_from(pid).ok().filter(|p| *p > 0);
        let reading = if class.is_empty() && pid.is_none() {
            FocusReading::Nothing
        } else {
            FocusReading::App(FocusedApp { app_id: class, pid })
        };
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = reading;
    }
}

pub struct Kwin {
    conn: Connection,
    latest: Latest,
}

impl Kwin {
    pub fn start() -> Option<Self> {
        let latest: Latest = Arc::new(Mutex::new(FocusReading::Unknown));
        let conn = Builder::session()
            .ok()?
            .name(BUS_NAME)
            .ok()?
            .serve_at(OBJECT, Sink(latest.clone()))
            .ok()?
            .build()
            .ok()?;
        let kwin = Self { conn, latest };
        kwin.load_script()?;
        Some(kwin)
    }

    fn load_script(&self) -> Option<()> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("sisyphus");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("kwin-focus.js");
        std::fs::write(&path, SCRIPT).ok()?;

        // A script from an earlier run that didn't shut down cleanly.
        self.unload();
        let reply = self
            .conn
            .call_method(
                Some("org.kde.KWin"),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "loadScript",
                &(path.to_str()?, PLUGIN),
            )
            .ok()?;
        let id: i32 = reply.body().deserialize().ok()?;
        if id < 0 {
            return None;
        }
        // Plasma 6 puts scripts under /Scripting/Script<id>, Plasma 5
        // under /<id>.
        for object in [format!("/Scripting/Script{id}"), format!("/{id}")] {
            let ran = self.conn.call_method(
                Some("org.kde.KWin"),
                object.as_str(),
                Some("org.kde.kwin.Script"),
                "run",
                &(),
            );
            if ran.is_ok() {
                return Some(());
            }
        }
        None
    }

    fn unload(&self) {
        let _ = self.conn.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(PLUGIN,),
        );
    }
}

impl Drop for Kwin {
    fn drop(&mut self) {
        self.unload();
    }
}

impl FocusSource for Kwin {
    fn focused(&mut self) -> FocusReading {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn name(&self) -> &'static str {
        "KWin script"
    }
}
