//! The client's local settings, stored as JSON in the user's config
//! directory:
//!
//! - Linux: `$XDG_CONFIG_HOME/sisyphus/config.json` (`~/.config/...`)
//! - macOS: `~/Library/Application Support/sisyphus/config.json`
//! - Windows: `%APPDATA%\sisyphus\config.json`

use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Ids of the agent integrations the user connected
    /// (see [`crate::agents::Integration::id`]).
    pub connected_agents: Vec<String>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        let base = if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        }?;
        Some(base.join("sisyphus").join("config.json"))
    }

    /// Load the config. A missing or unreadable file gives the defaults.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| Self::from_json(&v))
            .unwrap_or_default()
    }

    /// Write the config, replacing the file atomically.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(&self.to_json()).map_err(io::Error::other)?;
        std::fs::write(&tmp, body + "\n")?;
        std::fs::rename(tmp, path)
    }

    pub fn is_connected(&self, agent_id: &str) -> bool {
        self.connected_agents.iter().any(|a| a == agent_id)
    }

    pub fn set_connected(&mut self, agent_id: &str, connected: bool) {
        self.connected_agents.retain(|a| a != agent_id);
        if connected {
            self.connected_agents.push(agent_id.to_string());
        }
    }

    fn from_json(v: &Value) -> Self {
        let connected_agents = v
            .get("connected_agents")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self { connected_agents }
    }

    fn to_json(&self) -> Value {
        json!({ "connected_agents": self.connected_agents })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_and_disconnect() {
        let mut c = Config::default();
        c.set_connected("opencode", true);
        c.set_connected("opencode", true);
        assert_eq!(c.connected_agents, vec!["opencode"]);
        assert!(c.is_connected("opencode"));
        c.set_connected("opencode", false);
        assert!(!c.is_connected("opencode"));
    }

    #[test]
    fn json_round_trip_ignores_unknown_fields() {
        let mut c = Config::default();
        c.set_connected("claude-code", true);
        assert_eq!(Config::from_json(&c.to_json()), c);
        let v = json!({ "connected_agents": ["a", 3], "other": true });
        assert_eq!(Config::from_json(&v).connected_agents, vec!["a"]);
    }
}
