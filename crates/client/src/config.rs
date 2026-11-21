//! The client's local settings, stored as JSON in the user's config
//! directory:
//!
//! - Linux: `$XDG_CONFIG_HOME/sisyphus/config.json` (`~/.config/...`)
//! - macOS: `~/Library/Application Support/sisyphus/config.json`
//! - Windows: `%APPDATA%\sisyphus\config.json`

use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};

/// Where the Sisyphus server is, unless the config says otherwise.
pub const DEFAULT_SERVER_URL: &str = "https://sisyphus.aelx.dev";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Ids of the agent integrations the user connected
    /// (see [`crate::agents::Integration::id`]).
    pub connected_agents: Vec<String>,
    /// Dark mode chosen in the window. `None` follows the OS.
    pub dark_mode: Option<bool>,
    /// The Sisyphus server; `None` uses [`DEFAULT_SERVER_URL`].
    pub server_url: Option<String>,
    /// The account this device was linked to, once logged in. Use
    /// [`Config::account`], which only returns it for the current server.
    pub account: Option<Account>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Account {
    /// Display name from Hack Club, if it has one.
    pub name: Option<String>,
    /// The server the device was linked on. A device is linked per server,
    /// so the account only counts while this is the current server.
    pub server: Option<String>,
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

    /// The Sisyphus server's URL, without a trailing slash.
    pub fn server_url(&self) -> String {
        self.server_url
            .as_deref()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or(DEFAULT_SERVER_URL)
            .trim_end_matches('/')
            .to_string()
    }

    /// The logged-in account, if it belongs to the current server.
    pub fn account(&self) -> Option<&Account> {
        let server = self.server_url();
        self.account.as_ref().filter(|a| {
            a.server.as_deref().map(|s| s.trim_end_matches('/')) == Some(server.as_str())
        })
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
        let dark_mode = v.get("dark_mode").and_then(Value::as_bool);
        let server_url = v
            .get("server_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        let account = v.get("account").filter(|a| a.is_object()).map(|a| Account {
            name: a.get("name").and_then(Value::as_str).map(str::to_string),
            server: a.get("server").and_then(Value::as_str).map(str::to_string),
        });
        Self {
            connected_agents,
            dark_mode,
            server_url,
            account,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "connected_agents": self.connected_agents,
            "dark_mode": self.dark_mode,
            "server_url": self.server_url,
            "account": self
                .account
                .as_ref()
                .map(|a| json!({ "name": a.name, "server": a.server })),
        })
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
        c.dark_mode = Some(true);
        c.server_url = Some("https://sisyphus.example/".to_string());
        c.account = Some(Account {
            name: Some("Ada".to_string()),
            server: Some("https://sisyphus.example".to_string()),
        });
        assert_eq!(Config::from_json(&c.to_json()), c);
        assert_eq!(c.server_url(), "https://sisyphus.example");
        assert_eq!(Config::default().server_url(), DEFAULT_SERVER_URL);
        let v = json!({ "connected_agents": ["a", 3], "other": true });
        assert_eq!(Config::from_json(&v).connected_agents, vec!["a"]);
    }

    #[test]
    fn account_only_counts_on_its_server() {
        let mut c = Config {
            account: Some(Account {
                name: Some("Ada".to_string()),
                server: Some(DEFAULT_SERVER_URL.to_string()),
            }),
            ..Config::default()
        };
        assert!(c.account().is_some());
        c.server_url = Some("http://localhost:8080".to_string());
        assert!(c.account().is_none());
        // Accounts saved before the server was recorded don't count.
        c.account = Some(Account::default());
        assert!(c.account().is_none());
    }
}
