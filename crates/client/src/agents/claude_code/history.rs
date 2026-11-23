//! Paste counts from Claude Code's prompt history.
//!
//! Claude Code appends every submitted prompt to `~/.claude/history.jsonl`:
//!
//! ```json
//! {"display": "fix this [Pasted text #1 +40 lines]", "pastedContents": {"1": {...}},
//!  "timestamp": 1795126580750, "project": "/path", "sessionId": "<uuid>"}
//! ```
//!
//! `display` is the prompt as shown in the input box, where large pastes are
//! collapsed into a placeholder. Removing the placeholders leaves what was
//! typed. The transcript has the expanded prompt, so pasted = prompt length
//! minus typed length. When Claude Code wraps the pasted text in
//! `<pasted_content>` tags inside the prompt, the text inside the tags is
//! counted instead, so the tags themselves don't count as pasted.
//!
//! History entries are matched to transcript prompts by session id and
//! timestamp; both are written within milliseconds of each other. A resumed
//! session can log its prompts under a different session id, so if nothing
//! matches in the same session, an entry from any session at the same time
//! is used, as long as it is the only one whose length fits the prompt.
//! Queued prompts (sent while the agent is busy) are usually not in the
//! history at all. Only lengths are kept, never the pasted text.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents::{AgentEvent, AgentEventKind, FileStamp};

/// How far apart a history entry and a transcript prompt may be.
const MATCH_WINDOW_MS: u64 = 2_000;

const PLACEHOLDERS: [&str; 2] = ["[Pasted text #", "[Image #"];

#[derive(Debug, Clone)]
struct Entry {
    session_id: String,
    time_ms: u64,
    typed_chars: usize,
    has_paste: bool,
}

#[derive(Debug, Default)]
pub struct PromptHistory {
    path: Option<PathBuf>,
    stamp: Option<FileStamp>,
    /// Sorted by time.
    entries: Vec<Entry>,
}

impl PromptHistory {
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
        self.entries.clear();
        let Ok(file) = File::open(path) else { return };
        self.entries = BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| parse_entry(&line))
            .collect();
        self.entries.sort_by_key(|e| e.time_ms);
    }

    /// Pasted characters in `prompt`, sent in `session_id` at `time_ms`.
    /// Re-reads the file once if the prompt isn't found and the file has
    /// changed, since a live prompt may be newer than the loaded history.
    pub fn pasted_chars(&mut self, session_id: &str, time_ms: u64, prompt: &str) -> Option<usize> {
        let prompt_chars = prompt.chars().count();
        if self.find(session_id, time_ms, prompt_chars).is_none()
            && self.path.as_deref().and_then(FileStamp::of) != self.stamp
        {
            self.reload();
        }
        let entry = self.find(session_id, time_ms, prompt_chars)?;
        Some(pasted_chars(entry, prompt))
    }

    /// Fill in `pasted_chars` on every prompt in `events`.
    pub fn fill(&mut self, session_id: &str, events: &mut [AgentEvent]) {
        for event in events {
            if let AgentEventKind::Prompt {
                text, pasted_chars, ..
            } = &mut event.kind
            {
                *pasted_chars = self.pasted_chars(session_id, event.time_ms, text);
            }
        }
    }

    fn find(&self, session_id: &str, time_ms: u64, prompt_chars: usize) -> Option<&Entry> {
        let start = self
            .entries
            .partition_point(|e| e.time_ms < time_ms.saturating_sub(MATCH_WINDOW_MS));
        let nearby: Vec<&Entry> = self.entries[start..]
            .iter()
            .take_while(|e| e.time_ms <= time_ms + MATCH_WINDOW_MS)
            .collect();

        let same_session = nearby
            .iter()
            .filter(|e| e.session_id == session_id)
            .min_by_key(|e| e.time_ms.abs_diff(time_ms));
        if let Some(entry) = same_session {
            return Some(entry);
        }
        // Without a paste the typed text is the whole prompt; with one it
        // can only be shorter.
        let mut fitting = nearby.into_iter().filter(|e| {
            if e.has_paste {
                e.typed_chars <= prompt_chars
            } else {
                e.typed_chars == prompt_chars
            }
        });
        match (fitting.next(), fitting.next()) {
            (Some(entry), None) => Some(entry),
            _ => None,
        }
    }
}

fn parse_entry(line: &str) -> Option<Entry> {
    let v: Value = serde_json::from_str(line).ok()?;
    let session_id = v.get("sessionId")?.as_str()?.to_string();
    let time_ms = v.get("timestamp")?.as_u64()?;
    let display = v.get("display")?.as_str()?;
    let has_paste = v
        .get("pastedContents")
        .and_then(Value::as_object)
        .is_some_and(|p| !p.is_empty());
    let typed_chars = strip_placeholders(display).chars().count();
    Some(Entry {
        session_id,
        time_ms,
        typed_chars,
        has_paste,
    })
}

/// `display` without paste and image placeholders.
fn strip_placeholders(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    let mut rest = display;
    loop {
        let next = PLACEHOLDERS
            .iter()
            .filter_map(|p| rest.find(p))
            .min()
            .and_then(|start| Some((start, start + rest[start..].find(']')? + 1)));
        match next {
            Some((start, end)) => {
                out.push_str(&rest[..start]);
                rest = &rest[end..];
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

fn pasted_chars(entry: &Entry, prompt: &str) -> usize {
    if !entry.has_paste {
        return 0;
    }
    tagged_paste_chars(prompt)
        .unwrap_or_else(|| prompt.chars().count().saturating_sub(entry.typed_chars))
}

/// Characters inside `<pasted_content ...>...</pasted_content ...>` tags,
/// or `None` if the prompt has none.
fn tagged_paste_chars(prompt: &str) -> Option<usize> {
    const OPEN: &str = "<pasted_content";
    const CLOSE: &str = "</pasted_content";
    let mut total = None;
    let mut rest = prompt;
    while let Some(open) = rest.find(OPEN) {
        let after = &rest[open..];
        let Some(body_start) = after.find('>').map(|i| i + 1) else {
            break;
        };
        let Some(body_len) = after[body_start..].find(CLOSE) else {
            break;
        };
        let body = &after[body_start..body_start + body_len];
        let body = body.strip_prefix('\n').unwrap_or(body);
        let body = body.strip_suffix('\n').unwrap_or(body);
        *total.get_or_insert(0) += body.chars().count();
        rest = &after[body_start + body_len + CLOSE.len()..];
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(lines: &[&str]) -> PromptHistory {
        let mut entries: Vec<Entry> = lines.iter().map(|l| parse_entry(l).unwrap()).collect();
        entries.sort_by_key(|e| e.time_ms);
        PromptHistory {
            entries,
            ..PromptHistory::default()
        }
    }

    #[test]
    fn other_session_only_if_unambiguous() {
        let mut h = history(&[
            r#"{"display":"hello","pastedContents":{},"timestamp":1000,"sessionId":"old"}"#,
            r#"{"display":"something longer","pastedContents":{},"timestamp":1500,"sessionId":"x"}"#,
        ]);
        // Resumed session: logged under "old", only "hello" fits.
        assert_eq!(h.pasted_chars("new", 1001, "hello"), Some(0));
        // Two entries could fit a prompt with a paste: no answer.
        h.entries.push(parse_entry(r#"{"display":"[Pasted text #1]","pastedContents":{"1":{}},"timestamp":1200,"sessionId":"y"}"#).unwrap());
        h.entries.push(parse_entry(r#"{"display":"a [Pasted text #1]","pastedContents":{"1":{}},"timestamp":1300,"sessionId":"z"}"#).unwrap());
        h.entries.sort_by_key(|e| e.time_ms);
        assert_eq!(h.pasted_chars("new", 1250, "a pasted text here"), None);
    }

    #[test]
    fn placeholders_are_not_typed() {
        assert_eq!(
            strip_placeholders("fix [Pasted text #1 +40 lines] now"),
            "fix  now"
        );
        assert_eq!(strip_placeholders("[Image #2][Pasted text #1]"), "");
        assert_eq!(strip_placeholders("a [b] c"), "a [b] c");
    }

    #[test]
    fn untagged_paste_is_prompt_minus_typed() {
        let mut h = history(&[
            r#"{"display":"ab [Pasted text #1 +2 lines]","pastedContents":{"1":{"id":1,"type":"text"}},"timestamp":1000,"sessionId":"s"}"#,
        ]);
        assert_eq!(h.pasted_chars("s", 1003, "ab 0123456789"), Some(10));
    }

    #[test]
    fn tagged_paste_counts_tag_contents() {
        let mut h = history(&[
            r#"{"display":"ab [Pasted text #1]","pastedContents":{"1":{"id":1,"type":"text"}},"timestamp":1000,"sessionId":"s"}"#,
        ]);
        let prompt = "ab \n\n<pasted_content id=\"x\">\nhello\n</pasted_content id=\"x\">";
        assert_eq!(h.pasted_chars("s", 999, prompt), Some(5));
    }

    #[test]
    fn no_paste_and_no_match() {
        let mut h =
            history(&[r#"{"display":"hi","pastedContents":{},"timestamp":1000,"sessionId":"s"}"#]);
        assert_eq!(h.pasted_chars("s", 1000, "hi"), Some(0));
        assert_eq!(h.pasted_chars("s", 9000, "hi"), None);
        // Another session at the same time, but the length doesn't fit.
        assert_eq!(h.pasted_chars("other", 1000, "hello"), None);
    }
}
