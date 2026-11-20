//! Paste counts from opencode's prompt history.
//!
//! opencode keeps recent prompts (about the last 50) in
//! `$XDG_STATE_HOME/opencode/prompt-history.jsonl`:
//!
//! ```json
//! {"text": "fix [Pasted ~13 lines]", "pasted": [{"text": "<pasted text>",
//!  "source": {"start": 4, "end": 22, "text": "[Pasted ~13 lines]"}}], ...}
//! ```
//!
//! Large pastes appear in `text` as a placeholder; `pasted` holds what each
//! placeholder stands for. Replacing the placeholders gives exactly the
//! prompt stored in the session database. The history has no session id or
//! timestamp, so prompts are matched by their full text. Older prompts have
//! dropped out of the history and get no count.
//!
//! Only a hash of each expanded prompt and its paste count are kept, never
//! the text.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents::{AgentEvent, AgentEventKind, FileStamp};

#[derive(Debug, Default)]
pub struct PromptHistory {
    path: Option<PathBuf>,
    stamp: Option<FileStamp>,
    /// Pasted characters by hash of the expanded prompt.
    pasted: HashMap<u64, usize>,
}

impl PromptHistory {
    /// Path of opencode's prompt history.
    pub fn default_path() -> Option<PathBuf> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
        Some(state.join("opencode").join("prompt-history.jsonl"))
    }

    /// Read the history file. A missing or unreadable file gives an empty
    /// history, so every prompt's paste count is unknown.
    pub fn load(path: &Path) -> Self {
        let mut history = Self {
            path: Some(path.to_path_buf()),
            ..Self::default()
        };
        history.reload();
        history
    }

    fn reload(&mut self) {
        let Some(path) = &self.path else { return };
        self.stamp = FileStamp::of(path);
        self.pasted.clear();
        let Ok(file) = File::open(path) else { return };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some((prompt, pasted)) = parse_entry(&line) {
                self.insert(&prompt, pasted);
            }
        }
    }

    /// The same prompt can be in the history twice: once with its paste,
    /// and again as plain text when it is re-sent from the history (arrow
    /// up). Re-sending isn't typing, so the larger count wins.
    fn insert(&mut self, prompt: &str, pasted: usize) {
        let count = self.pasted.entry(hash(prompt)).or_insert(0);
        *count = (*count).max(pasted);
    }

    /// Pasted characters in `prompt`. Re-reads the file once if the prompt
    /// isn't found and the file has changed.
    pub fn pasted_chars(&mut self, prompt: &str) -> Option<usize> {
        let key = hash(prompt);
        if !self.pasted.contains_key(&key)
            && self.path.as_deref().and_then(FileStamp::of) != self.stamp
        {
            self.reload();
        }
        self.pasted.get(&key).copied()
    }

    /// Fill in `pasted_chars` on every prompt in `events`.
    pub fn fill(&mut self, events: &mut [AgentEvent]) {
        for event in events {
            if let AgentEventKind::Prompt { text, pasted_chars } = &mut event.kind {
                *pasted_chars = self.pasted_chars(text);
            }
        }
    }
}

fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The expanded prompt and its pasted character count.
fn parse_entry(line: &str) -> Option<(String, usize)> {
    let v: Value = serde_json::from_str(line).ok()?;
    let text = v.get("text")?.as_str()?;
    let mut pastes: Vec<(&str, &str, u64)> = v
        .get("pasted")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| {
            let content = p.get("text")?.as_str()?;
            let placeholder = p.pointer("/source/text")?.as_str()?;
            let start = p
                .pointer("/source/start")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Some((placeholder, content, start))
        })
        .collect();
    pastes.sort_by_key(|&(.., start)| start);

    // `source.start` may count UTF-16 units (opencode is TypeScript), so
    // only use it for ordering and find each placeholder by its text.
    let mut expanded = String::with_capacity(text.len());
    let mut rest = text;
    let mut pasted = 0;
    for (placeholder, content, _) in pastes {
        let at = rest.find(placeholder)?;
        expanded.push_str(&rest[..at]);
        expanded.push_str(content);
        pasted += content.chars().count();
        rest = &rest[at + placeholder.len()..];
    }
    expanded.push_str(rest);
    Some((expanded, pasted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(lines: &[&str]) -> PromptHistory {
        let mut h = PromptHistory::default();
        for line in lines {
            let (prompt, pasted) = parse_entry(line).unwrap();
            h.insert(&prompt, pasted);
        }
        h
    }

    #[test]
    fn expands_placeholders() {
        let mut h = history(&[
            r#"{"text":"fix [Pasted ~2 lines] and [Pasted ~1 lines]","pasted":[
                {"text":"bb","source":{"start":26,"end":43,"text":"[Pasted ~1 lines]"}},
                {"text":"a\na","source":{"start":4,"end":21,"text":"[Pasted ~2 lines]"}}]}"#,
        ]);
        assert_eq!(h.pasted_chars("fix a\na and bb"), Some(5));
    }

    #[test]
    fn resent_prompt_keeps_paste_count() {
        let mut h = history(&[
            r#"{"text":"[Pasted ~1 lines]","pasted":[{"text":"abc","source":{"start":0,"end":17,"text":"[Pasted ~1 lines]"}}]}"#,
            r#"{"text":"abc","pasted":[]}"#,
        ]);
        assert_eq!(h.pasted_chars("abc"), Some(3));
    }

    #[test]
    fn typed_prompt_and_unknown_prompt() {
        let mut h = history(&[r#"{"text":"hello","pasted":[]}"#]);
        assert_eq!(h.pasted_chars("hello"), Some(0));
        assert_eq!(h.pasted_chars("not in history"), None);
    }

    #[test]
    fn missing_placeholder_is_skipped() {
        let line = r#"{"text":"edited","pasted":[{"text":"x","source":{"start":0,"end":5,"text":"[Pasted]"}}]}"#;
        assert_eq!(parse_entry(line), None);
    }
}
