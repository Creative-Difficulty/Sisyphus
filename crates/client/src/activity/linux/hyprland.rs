//! Hyprland: ask its IPC socket for the active window.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use super::FocusSource;
use crate::activity::{FocusReading, FocusedApp};

pub struct Hyprland {
    socket: PathBuf,
}

impl Hyprland {
    pub fn detect() -> Option<Self> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        // Newer versions keep the socket in the runtime dir, older in /tmp.
        let candidates = runtime
            .into_iter()
            .map(|r| r.join("hypr"))
            .chain([PathBuf::from("/tmp/hypr")])
            .map(|dir| dir.join(&signature).join(".socket.sock"));
        let socket = candidates.into_iter().find(|p| p.exists())?;
        Some(Self { socket })
    }

    fn read(&self) -> Option<FocusReading> {
        let mut stream = UnixStream::connect(&self.socket).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
        stream.write_all(b"j/activewindow").ok()?;
        let mut body = Vec::new();
        stream.take(1 << 20).read_to_end(&mut body).ok()?;
        parse(&body)
    }
}

/// The reply is the window as JSON, or `{}` when nothing has focus. Only
/// the class and pid are kept.
fn parse(body: &[u8]) -> Option<FocusReading> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let class = v.get("class").and_then(Value::as_str);
    let Some(class) = class.filter(|c| !c.is_empty()) else {
        return Some(FocusReading::Nothing);
    };
    let pid = v
        .get("pid")
        .and_then(Value::as_i64)
        .and_then(|p| u32::try_from(p).ok());
    Some(FocusReading::App(FocusedApp {
        app_id: class.to_string(),
        pid,
    }))
}

impl FocusSource for Hyprland {
    fn focused(&mut self) -> FocusReading {
        self.read().unwrap_or(FocusReading::Unknown)
    }

    fn name(&self) -> &'static str {
        "Hyprland IPC"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_active_window() {
        let body = br#"{"address":"0x1","class":"kitty","title":"secret","pid":4242}"#;
        assert_eq!(
            parse(body),
            Some(FocusReading::App(FocusedApp {
                app_id: "kitty".to_string(),
                pid: Some(4242),
            }))
        );
        assert_eq!(parse(b"{}"), Some(FocusReading::Nothing));
        assert_eq!(parse(b"not json"), None);
    }
}
