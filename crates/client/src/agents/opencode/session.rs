//! Turns opencode v2 session messages into [`AgentEvent`]s.
//!
//! opencode keeps each session as rows in the `session_message` table of its
//! SQLite database, ordered by `seq`. The `data` column is JSON, shaped by
//! the row's `type`:
//!
//! - `user`: `{ time: { created }, text, files, agents }`, a prompt.
//! - `assistant`: one model step. `content` is a list of `reasoning`, `text`
//!   and `tool` blocks; `time.completed` is set when the step ends, `finish`
//!   says why (`tool-calls` means the agent keeps going), and `error` is set
//!   when it failed or was aborted (`error.type == "aborted"`). `model.id`
//!   names the model.
//! - `compaction`: the conversation was summarized.
//! - `synthetic`, `system`, `idle`, `model-switched`, ...: generated or
//!   bookkeeping rows, ignored.
//!
//! Unlike Claude Code's append-only transcript, assistant rows are rewritten
//! in place while the step streams. The parser can be fed the same row many
//! times and remembers what it already emitted, so each event comes out once
//! and only when it is complete.

use std::collections::HashSet;

use serde_json::Value;

use crate::agents::{AgentEvent, AgentEventKind, ToolOutcome};

#[derive(Debug, Default)]
pub struct SessionParser {
    /// Keys of events already emitted, e.g. `msg_1:tool-result:3`.
    emitted: HashSet<String>,
    title: Option<String>,
    /// When the current turn's prompt was sent, for turn durations.
    last_prompt_ms: Option<u64>,
}

impl SessionParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the session's current title. Emits an event when it changed.
    pub fn set_title(&mut self, title: Option<&str>, time_ms: u64) -> Option<AgentEvent> {
        let title = title.filter(|t| !t.is_empty())?;
        if self.title.as_deref() == Some(title) {
            return None;
        }
        self.title = Some(title.to_string());
        Some(AgentEvent {
            time_ms,
            kind: AgentEventKind::TitleChanged {
                title: title.to_string(),
            },
        })
    }

    /// Feed one `session_message` row, as often as it changes.
    pub fn process(
        &mut self,
        id: &str,
        kind: &str,
        time_updated: u64,
        data: &str,
    ) -> Vec<AgentEvent> {
        let Ok(data) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let created = data
            .pointer("/time/created")
            .and_then(Value::as_u64)
            .unwrap_or(time_updated);
        let mut out = Emitter {
            parser: self,
            id,
            events: Vec::new(),
        };

        match kind {
            "user" => {
                let text = data.get("text").and_then(Value::as_str).unwrap_or("");
                if !text.trim().is_empty()
                    && out.once("prompt", created, || AgentEventKind::Prompt {
                        text: text.to_string(),
                        // Filled in from the prompt history, see `history.rs`.
                        pasted_chars: None,
                        scripted: false,
                    })
                {
                    out.parser.last_prompt_ms = Some(created);
                }
            }
            "assistant" => out.assistant(&data, time_updated),
            "compaction" => {
                out.once("compaction", created, || AgentEventKind::Compacted);
            }
            _ => {}
        }
        out.events
    }
}

/// Emits events for one row, each at most once per session.
struct Emitter<'a> {
    parser: &'a mut SessionParser,
    id: &'a str,
    events: Vec<AgentEvent>,
}

impl Emitter<'_> {
    /// Emit `kind` unless this row already emitted `key`. True if emitted.
    fn once(&mut self, key: &str, time_ms: u64, kind: impl FnOnce() -> AgentEventKind) -> bool {
        if !self.parser.emitted.insert(format!("{}:{key}", self.id)) {
            return false;
        }
        self.events.push(AgentEvent {
            time_ms,
            kind: kind(),
        });
        true
    }

    fn assistant(&mut self, data: &Value, time_updated: u64) {
        let completed = data.pointer("/time/completed").and_then(Value::as_u64);
        let error = data.get("error").filter(|e| !e.is_null());
        let done = completed.is_some() || error.is_some();
        let end_ms = completed.unwrap_or(time_updated);
        let blocks = data
            .get("content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        if let Some(model) = data
            .pointer("/model/id")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
        {
            let created = data
                .pointer("/time/created")
                .and_then(Value::as_u64)
                .unwrap_or(time_updated);
            self.once("model", created, || AgentEventKind::Model {
                name: model.to_string(),
            });
        }

        for (i, block) in blocks.iter().enumerate() {
            let field = |name| block.get(name).and_then(Value::as_str).unwrap_or("");
            let block_time = |name: &str| {
                block
                    .pointer(&format!("/time/{name}"))
                    .and_then(Value::as_u64)
            };
            match field("type") {
                // Reasoning blocks say when they finished; text blocks don't,
                // so those wait for the whole step. Reasoning is timestamped
                // when it started, so it sorts before what it led to.
                "reasoning" if block_time("completed").is_some() || done => {
                    let t = block_time("created").unwrap_or(end_ms);
                    self.once(&format!("reasoning:{i}"), t, || AgentEventKind::Thinking {
                        text: field("text").to_string(),
                    });
                }
                "text" if done && !field("text").trim().is_empty() => {
                    self.once(&format!("text:{i}"), end_ms, || AgentEventKind::Response {
                        text: field("text").to_string(),
                    });
                }
                "tool" => self.tool(i, block, done, end_ms),
                _ => {}
            }
        }

        if !done {
            return;
        }
        if error.and_then(|e| e.get("type")).and_then(Value::as_str) == Some("aborted") {
            let during_tool_use = blocks.iter().any(|b| {
                tool_outcome(b).is_none()
                    || error_type(b) == Some("aborted")
                    || error_message(b) == Some("Tool execution aborted")
            });
            self.once("interrupt", end_ms, || AgentEventKind::Interrupt {
                during_tool_use,
            });
        } else if data.get("finish").and_then(Value::as_str) != Some("tool-calls") {
            let duration_ms = self.parser.last_prompt_ms.map(|p| end_ms.saturating_sub(p));
            self.once("turn-end", end_ms, || AgentEventKind::TurnEnd {
                duration_ms,
            });
        }
    }

    fn tool(&mut self, i: usize, block: &Value, done: bool, end_ms: u64) {
        let status = block
            .pointer("/state/status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let created = block
            .pointer("/time/created")
            .and_then(Value::as_u64)
            .unwrap_or(end_ms);
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // While `pending`, the model is still writing the input.
        if status != "pending" || done {
            self.once(&format!("tool-call:{i}"), created, || {
                AgentEventKind::ToolCall {
                    id: id.clone(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    input: block
                        .pointer("/state/input")
                        .cloned()
                        .unwrap_or(Value::Null),
                }
            });
        }
        if let Some(outcome) = tool_outcome(block) {
            let t = block
                .pointer("/time/completed")
                .and_then(Value::as_u64)
                .unwrap_or(end_ms);
            self.once(&format!("tool-result:{i}"), t, || {
                AgentEventKind::ToolResult { id, outcome }
            });
        }
    }
}

/// Outcome of a tool block, `None` while it is still running. Non-tool
/// blocks count as finished.
fn tool_outcome(block: &Value) -> Option<ToolOutcome> {
    if block.get("type").and_then(Value::as_str) != Some("tool") {
        return Some(ToolOutcome::Success);
    }
    match block.pointer("/state/status").and_then(Value::as_str)? {
        "completed" => Some(ToolOutcome::Success),
        "error" => {
            let rejected = error_type(block) == Some("permission.rejected")
                || error_message(block).is_some_and(|m| m.contains("user rejected"));
            Some(if rejected {
                ToolOutcome::Rejected
            } else {
                ToolOutcome::Error
            })
        }
        _ => None,
    }
}

fn error_type(block: &Value) -> Option<&str> {
    block.pointer("/state/error/type").and_then(Value::as_str)
}

/// The tool error's message. Older migrated sessions store it as a string.
fn error_message(block: &Value) -> Option<&str> {
    let error = block.pointer("/state/error")?;
    error
        .as_str()
        .or_else(|| error.get("message").and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kinds(events: Vec<AgentEvent>) -> Vec<AgentEventKind> {
        events.into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn prompt_once() {
        let mut p = SessionParser::new();
        let row = r#"{"time":{"created":5},"text":"Hi","files":[]}"#;
        assert_eq!(
            p.process("m1", "user", 5, row),
            vec![AgentEvent {
                time_ms: 5,
                kind: AgentEventKind::Prompt {
                    text: "Hi".into(),
                    pasted_chars: None,
                    scripted: false,
                }
            }]
        );
        assert_eq!(p.process("m1", "user", 6, row), vec![]);
    }

    #[test]
    fn ignores_generated_rows() {
        let mut p = SessionParser::new();
        for kind in ["synthetic", "system", "idle", "model-switched"] {
            assert_eq!(
                p.process("m", kind, 1, r#"{"time":{"created":1},"text":"Continue"}"#),
                vec![],
                "{kind}"
            );
        }
    }

    #[test]
    fn streaming_step_emits_each_event_once() {
        let mut p = SessionParser::new();
        p.process("u", "user", 100, r#"{"time":{"created":100},"text":"go"}"#);

        // Mid-stream: reasoning done, tool running, text still growing.
        let partial = json!({
            "time": {"created": 110},
            "content": [
                {"type": "reasoning", "text": "think", "time": {"created": 111, "completed": 112}},
                {"type": "tool", "id": "c1", "name": "bash", "time": {"created": 113},
                 "state": {"status": "running", "input": {"command": "ls"}}},
                {"type": "text", "text": "Do"},
            ]
        });
        assert_eq!(
            kinds(p.process("a", "assistant", 113, &partial.to_string())),
            vec![
                AgentEventKind::Thinking {
                    text: "think".into()
                },
                AgentEventKind::ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"})
                },
            ]
        );

        let finished = json!({
            "time": {"created": 110, "completed": 130},
            "finish": "stop",
            "content": [
                {"type": "reasoning", "text": "think", "time": {"created": 111, "completed": 112}},
                {"type": "tool", "id": "c1", "name": "bash", "time": {"created": 113, "completed": 120},
                 "state": {"status": "completed", "input": {"command": "ls"}}},
                {"type": "text", "text": "Done"},
            ]
        });
        assert_eq!(
            kinds(p.process("a", "assistant", 130, &finished.to_string())),
            vec![
                AgentEventKind::ToolResult {
                    id: "c1".into(),
                    outcome: ToolOutcome::Success
                },
                AgentEventKind::Response {
                    text: "Done".into()
                },
                AgentEventKind::TurnEnd {
                    duration_ms: Some(30)
                },
            ]
        );
        assert_eq!(
            p.process("a", "assistant", 131, &finished.to_string()),
            vec![]
        );
    }

    #[test]
    fn model_once_per_step() {
        let mut p = SessionParser::new();
        let row = r#"{"time":{"created":1},"model":{"id":"big-pickle","providerID":"opencode"},"content":[]}"#;
        let model = vec![AgentEvent {
            time_ms: 1,
            kind: AgentEventKind::Model {
                name: "big-pickle".into(),
            },
        }];
        assert_eq!(p.process("a", "assistant", 1, row), model);
        assert_eq!(p.process("a", "assistant", 2, row), vec![]);
    }

    #[test]
    fn tool_steps_do_not_end_the_turn() {
        let mut p = SessionParser::new();
        let row = r#"{"time":{"created":1,"completed":2},"finish":"tool-calls","content":[]}"#;
        assert_eq!(p.process("a", "assistant", 2, row), vec![]);
    }

    #[test]
    fn rejected_and_failed_tools() {
        let mut p = SessionParser::new();
        let row = json!({
            "time": {"created": 1, "completed": 2},
            "finish": "tool-calls",
            "content": [
                {"type": "tool", "id": "a", "name": "bash", "state": {"status": "error", "input": {},
                 "error": {"type": "permission.rejected", "message": "Permission denied: shell"}}},
                {"type": "tool", "id": "b", "name": "edit", "state": {"status": "error", "input": {},
                 "error": "Could not find oldString in the file."}},
            ]
        });
        let results: Vec<_> = kinds(p.process("m", "assistant", 2, &row.to_string()))
            .into_iter()
            .filter(|k| matches!(k, AgentEventKind::ToolResult { .. }))
            .collect();
        assert_eq!(
            results,
            vec![
                AgentEventKind::ToolResult {
                    id: "a".into(),
                    outcome: ToolOutcome::Rejected
                },
                AgentEventKind::ToolResult {
                    id: "b".into(),
                    outcome: ToolOutcome::Error
                },
            ]
        );
    }

    #[test]
    fn abort_is_an_interrupt() {
        let mut p = SessionParser::new();
        let row = json!({
            "time": {"created": 1, "completed": 9},
            "error": {"type": "aborted", "message": "Aborted"},
            "content": [
                {"type": "tool", "id": "a", "name": "bash", "state": {"status": "error", "input": {},
                 "error": {"type": "tool.execution", "message": "Tool execution aborted"}}},
            ]
        });
        let events = kinds(p.process("m", "assistant", 9, &row.to_string()));
        assert_eq!(
            events.last(),
            Some(&AgentEventKind::Interrupt {
                during_tool_use: true
            })
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEventKind::TurnEnd { .. }))
        );
    }

    #[test]
    fn compaction_and_title() {
        let mut p = SessionParser::new();
        assert_eq!(
            kinds(p.process(
                "c",
                "compaction",
                3,
                r#"{"time":{"created":3},"status":"completed"}"#
            )),
            vec![AgentEventKind::Compacted]
        );
        assert!(p.set_title(Some("A"), 1).is_some());
        assert!(p.set_title(Some("A"), 2).is_none());
        assert!(p.set_title(None, 2).is_none());
        assert!(p.set_title(Some("B"), 3).is_some());
    }
}
