//! Sway (and other compositors speaking the i3 IPC on `$SWAYSOCK`): find
//! the focused node in the tree.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use super::FocusSource;
use crate::activity::{FocusReading, FocusedApp};

const MAGIC: &[u8] = b"i3-ipc";
const GET_TREE: u32 = 4;
/// Trees are small; refuse anything absurd.
const MAX_REPLY: u32 = 16 << 20;

pub struct Sway {
    socket: PathBuf,
}

impl Sway {
    pub fn detect() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("SWAYSOCK")?);
        socket.exists().then_some(Self { socket })
    }

    fn read(&self) -> Option<FocusReading> {
        let mut stream = UnixStream::connect(&self.socket).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
        let mut request = MAGIC.to_vec();
        request.extend(0u32.to_ne_bytes());
        request.extend(GET_TREE.to_ne_bytes());
        stream.write_all(&request).ok()?;

        let mut header = [0u8; 14];
        stream.read_exact(&mut header).ok()?;
        if &header[..6] != MAGIC {
            return None;
        }
        let len = u32::from_ne_bytes(header[6..10].try_into().ok()?);
        if len > MAX_REPLY {
            return None;
        }
        let mut body = vec![0u8; len as usize];
        stream.read_exact(&mut body).ok()?;
        let tree: Value = serde_json::from_slice(&body).ok()?;
        Some(focused(&tree))
    }
}

/// The focused window in a sway tree. Only its app id (or X11 class) and
/// pid are kept.
fn focused(tree: &Value) -> FocusReading {
    let Some(node) = find_focused(tree) else {
        return FocusReading::Nothing;
    };
    let app_id = node.get("app_id").and_then(Value::as_str).or_else(|| {
        node.pointer("/window_properties/class")
            .and_then(Value::as_str)
    });
    let pid = node
        .get("pid")
        .and_then(Value::as_i64)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0);
    // A focused workspace or output has neither: no window has focus. A
    // window without an app id still has a pid.
    if app_id.is_none() && pid.is_none() {
        return FocusReading::Nothing;
    }
    FocusReading::App(FocusedApp {
        app_id: app_id.unwrap_or_default().to_string(),
        pid,
    })
}

fn find_focused(node: &Value) -> Option<&Value> {
    if node.get("focused").and_then(Value::as_bool) == Some(true) {
        return Some(node);
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|key| node.get(key).and_then(Value::as_array))
        .flatten()
        .find_map(find_focused)
}

impl FocusSource for Sway {
    fn focused(&mut self) -> FocusReading {
        self.read().unwrap_or(FocusReading::Unknown)
    }

    fn name(&self) -> &'static str {
        "Sway IPC"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_focused_window() {
        let tree = serde_json::json!({
            "focused": false,
            "nodes": [{
                "focused": false,
                "nodes": [
                    { "focused": false, "app_id": "firefox", "pid": 1 },
                ],
                "floating_nodes": [
                    { "focused": true, "app_id": null, "pid": 7,
                      "window_properties": { "class": "Code", "title": "secret" } },
                ],
            }],
        });
        assert_eq!(
            focused(&tree),
            FocusReading::App(FocusedApp {
                app_id: "Code".to_string(),
                pid: Some(7),
            })
        );
        let empty = serde_json::json!({ "focused": false, "nodes": [{ "focused": true }] });
        assert_eq!(focused(&empty), FocusReading::Nothing);
    }
}
