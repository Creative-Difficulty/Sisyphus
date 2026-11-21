//! Session titles from Codex's `session_index.jsonl`.
//!
//! Codex keeps thread names (generated, or set with `/rename`) in
//! `$CODEX_HOME/session_index.jsonl`, one `{id, thread_name, updated_at}`
//! object per line, appending a new line whenever a name changes. The
//! rollout files themselves don't contain titles.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::agents::time::parse_timestamp_ms;

/// A thread's current name and when it was set (unix ms, if known).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Title {
    pub name: String,
    pub updated_ms: Option<u64>,
}

/// The current title of every thread in the index. Empty if the index
/// doesn't exist or can't be read.
#[derive(Debug, Default, Clone)]
pub struct Titles(HashMap<String, Title>);

impl Titles {
    pub fn load(index: &Path) -> Self {
        let Ok(file) = File::open(index) else {
            return Self::default();
        };
        let mut titles: HashMap<String, Title> = HashMap::new();
        for line in BufReader::new(file).split(b'\n').map_while(Result::ok) {
            let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            let text = |name| entry.get(name).and_then(Value::as_str);
            let (Some(id), Some(name)) = (text("id"), text("thread_name")) else {
                continue;
            };
            if name.trim().is_empty() {
                continue;
            }
            let title = Title {
                name: name.to_string(),
                updated_ms: text("updated_at").and_then(parse_timestamp_ms),
            };
            // The newest entry wins; on equal (or unknown) times, the later
            // line does.
            let newer = titles
                .get(id)
                .is_none_or(|old| title.updated_ms >= old.updated_ms);
            if newer {
                titles.insert(id.to_string(), title);
            }
        }
        Self(titles)
    }

    pub fn get(&self, thread_id: &str) -> Option<&Title> {
        self.0.get(thread_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_entry_wins() {
        let dir = std::env::temp_dir().join(format!("sisyphus-codex-index-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let index = dir.join("session_index.jsonl");
        std::fs::write(
            &index,
            [
                r#"{"id":"a","thread_name":"First","updated_at":"2026-11-21T02:00:00Z"}"#,
                r#"{"id":"b","thread_name":"Other","updated_at":"2026-11-21T02:00:00Z"}"#,
                r#"{"id":"a","thread_name":"Renamed","updated_at":"2026-11-21T03:00:00.5Z"}"#,
                r#"{"id":"a","thread_name":"Older line later","updated_at":"2026-11-21T01:00:00Z"}"#,
                r#"{"id":"b","thread_name":"","updated_at":"2026-11-21T04:00:00Z"}"#,
                "not json",
                r#"{"id":"c"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let titles = Titles::load(&index);
        assert_eq!(
            titles.get("a"),
            Some(&Title {
                name: "Renamed".into(),
                updated_ms: parse_timestamp_ms("2026-11-21T03:00:00.5Z"),
            })
        );
        assert_eq!(titles.get("b").map(|t| t.name.as_str()), Some("Other"));
        assert_eq!(titles.get("c"), None);
        assert!(Titles::load(&dir.join("missing.jsonl")).get("a").is_none());
    }
}
