//! Turns Codex rollout lines into [`AgentEvent`]s.
//!
//! Every line of a rollout is `{timestamp, type, payload}`. Two formats
//! exist, and one file uses only one of them:
//!
//! - legacy (sessions from Codex before 0.147): prompts, replies and
//!   reasoning are `event_msg` lines of type `user_message`,
//!   `agent_message` and `agent_reasoning`.
//! - paginated (the terminal UI since 0.147): they are `event_msg` lines of
//!   type `item_completed` whose `item` is a `UserMessage`, `AgentMessage`
//!   or `Reasoning`.
//!
//! Their record types don't overlap, so both are parsed without knowing
//! which one a file uses. Both have `event_msg` `task_complete` (a turn
//! ended) and a top-level `compacted` line. Injected context (environment,
//! instructions) arrives as `response_item` messages, which are not
//! prompts. The format is undocumented, so unknown lines and fields are
//! skipped rather than treated as errors.

use serde_json::Value;

use crate::agents::time::parse_timestamp_ms;
use crate::agents::{AgentEvent, AgentEventKind};

/// Parses one session's lines in order.
#[derive(Debug, Default)]
pub struct RolloutParser {
    last_time_ms: u64,
}

impl RolloutParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Events in one rollout line. Malformed lines produce none.
    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        if let Some(t) = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_timestamp_ms)
        {
            self.last_time_ms = t;
        }
        let time_ms = self.last_time_ms;
        let payload = entry.get("payload").unwrap_or(&Value::Null);
        let kinds = match entry.get("type").and_then(Value::as_str) {
            Some("event_msg") => event_msg(payload),
            // The paginated `ContextCompaction` item and the legacy
            // `context_compacted` event repeat this; only this line counts.
            Some("compacted") => vec![AgentEventKind::Compacted],
            _ => Vec::new(),
        };
        kinds
            .into_iter()
            .map(|kind| AgentEvent { time_ms, kind })
            .collect()
    }
}

fn event_msg(payload: &Value) -> Vec<AgentEventKind> {
    let text = |name| payload.get(name).and_then(Value::as_str).unwrap_or("");
    match payload.get("type").and_then(Value::as_str) {
        Some("task_complete" | "turn_complete") => vec![AgentEventKind::TurnEnd {
            duration_ms: payload.get("duration_ms").and_then(Value::as_u64),
        }],
        Some("user_message") => prompt(text("message")),
        Some("agent_message") => response(text("message")),
        Some("agent_reasoning") => vec![AgentEventKind::Thinking {
            text: text("text").to_string(),
        }],
        Some("item_completed") => item(payload.get("item").unwrap_or(&Value::Null)),
        _ => Vec::new(),
    }
}

/// A completed item of the paginated format.
fn item(item: &Value) -> Vec<AgentEventKind> {
    match item.get("type").and_then(Value::as_str) {
        Some("UserMessage") => prompt(&joined(item.get("content"), "text")),
        Some("AgentMessage") => response(&joined(item.get("content"), "text")),
        Some("Reasoning") => {
            let summary = strings(item.get("summary_text"));
            let text = if summary.is_empty() {
                strings(item.get("raw_content"))
            } else {
                summary
            };
            vec![AgentEventKind::Thinking { text }]
        }
        _ => Vec::new(),
    }
}

fn prompt(text: &str) -> Vec<AgentEventKind> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    // Codex expands pasted text into the prompt before saving it, so the
    // pasted part can't be told apart from what was typed.
    vec![AgentEventKind::Prompt {
        text: text.to_string(),
        pasted_chars: None,
    }]
}

fn response(text: &str) -> Vec<AgentEventKind> {
    vec![AgentEventKind::Response {
        text: text.to_string(),
    }]
}

/// The string `field` of every object in an array, joined by newlines.
fn joined(parts: Option<&Value>, field: &str) -> String {
    parts
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get(field).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The strings in an array, joined by newlines.
fn strings(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(p: &mut RolloutParser, line: &str) -> Vec<AgentEventKind> {
        p.parse_line(line).into_iter().map(|e| e.kind).collect()
    }

    fn prompt_kind(text: &str) -> AgentEventKind {
        AgentEventKind::Prompt {
            text: text.into(),
            pasted_chars: None,
        }
    }

    #[test]
    fn legacy_prompt_with_time() {
        let mut p = RolloutParser::new();
        let line = r#"{"timestamp":"1970-01-01T00:00:01.500Z","type":"event_msg","payload":{"type":"user_message","message":"fix it","images":[],"local_images":[],"text_elements":[]}}"#;
        assert_eq!(
            p.parse_line(line),
            vec![AgentEvent {
                time_ms: 1500,
                kind: prompt_kind("fix it")
            }]
        );
    }

    #[test]
    fn legacy_reply_and_reasoning() {
        let mut p = RolloutParser::new();
        let reasoning =
            r#"{"type":"event_msg","payload":{"type":"agent_reasoning","text":"**Checking**"}}"#;
        let reply = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"done"}}"#;
        assert_eq!(
            kinds(&mut p, reasoning),
            vec![AgentEventKind::Thinking {
                text: "**Checking**".into()
            }]
        );
        assert_eq!(
            kinds(&mut p, reply),
            vec![AgentEventKind::Response {
                text: "done".into()
            }]
        );
    }

    #[test]
    fn paginated_items() {
        let mut p = RolloutParser::new();
        let user = r#"{"type":"event_msg","payload":{"type":"item_completed","thread_id":"t","turn_id":"u","item":{"type":"UserMessage","id":"i","content":[{"type":"text","text":"hey","text_elements":[]},{"type":"local_image","path":"/x.png"}]}}}"#;
        let agent = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"m","content":[{"type":"Text","text":"hi"}]}}}"#;
        let summary = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","id":"r","summary_text":["short"],"raw_content":["long"]}}}"#;
        let raw = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","id":"r","summary_text":[],"raw_content":["long"]}}}"#;
        assert_eq!(kinds(&mut p, user), vec![prompt_kind("hey")]);
        assert_eq!(
            kinds(&mut p, agent),
            vec![AgentEventKind::Response { text: "hi".into() }]
        );
        let thinking = |t: &str| vec![AgentEventKind::Thinking { text: t.into() }];
        assert_eq!(kinds(&mut p, summary), thinking("short"));
        assert_eq!(kinds(&mut p, raw), thinking("long"));
    }

    #[test]
    fn turn_end() {
        let mut p = RolloutParser::new();
        let with = r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t","last_agent_message":"x","duration_ms":4155}}"#;
        let without =
            r#"{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":null}}"#;
        assert_eq!(
            kinds(&mut p, with),
            vec![AgentEventKind::TurnEnd {
                duration_ms: Some(4155)
            }]
        );
        assert_eq!(
            kinds(&mut p, without),
            vec![AgentEventKind::TurnEnd { duration_ms: None }]
        );
    }

    #[test]
    fn compaction_counts_once() {
        let mut p = RolloutParser::new();
        let compacted = r#"{"type":"compacted","payload":{"message":"","replacement_history":[]}}"#;
        let legacy_event = r#"{"type":"event_msg","payload":{"type":"context_compacted"}}"#;
        let item = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"ContextCompaction","id":"k"}}}"#;
        assert_eq!(kinds(&mut p, compacted), vec![AgentEventKind::Compacted]);
        assert_eq!(kinds(&mut p, legacy_event), vec![]);
        assert_eq!(kinds(&mut p, item), vec![]);
    }

    #[test]
    fn injected_context_and_junk_are_ignored() {
        let mut p = RolloutParser::new();
        for line in [
            "",
            "not json",
            "[1,2]",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>…</environment_context>"}]}}"#,
            r#"{"type":"session_meta","payload":{"id":"x","cwd":"/p"}}"#,
            r#"{"type":"turn_context","payload":{"cwd":"/p"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"   "}}"#,
            r#"{"type":"event_msg","payload":{"type":"item_completed"}}"#,
            r#"{"type":"event_msg"}"#,
        ] {
            assert_eq!(kinds(&mut p, line), vec![], "{line}");
        }
    }

    #[test]
    fn lines_without_time_keep_the_last_one() {
        let mut p = RolloutParser::new();
        p.parse_line(r#"{"timestamp":"1970-01-01T00:00:02Z","type":"turn_context","payload":{}}"#);
        let ev = p
            .parse_line(r#"{"type":"event_msg","payload":{"type":"agent_message","message":"x"}}"#);
        assert_eq!(ev[0].time_ms, 2000);
    }
}
