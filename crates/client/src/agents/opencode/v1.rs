//! Sessions from opencode versions before the v2 tables.
//!
//! These keep sessions in `session`, one row per prompt or model reply in
//! `message`, and that message's content in `part` rows. `message.data` is
//! JSON with a `role` (`user` or `assistant`), `time.created`, and for
//! replies `time.completed`, `finish` and `error`. `part.data` is JSON
//! shaped by its `type`:
//!
//! - `text`: prompt or reply text. Parts marked `synthetic` or `ignored`
//!   were added by opencode, not typed by the user.
//! - `reasoning`: `text` and `time.start` / `time.end`.
//! - `tool`: `callID`, `tool` (the name) and `state`, with `status`,
//!   `input`, `error` (a string) and `time.start` / `time.end`.
//! - `compaction`: on a user message, marks that the conversation was
//!   summarized. The reply to it has `summary: true` and holds the summary.
//! - `step-start`, `step-finish`, `patch`, `file`, ...: ignored.
//!
//! Each message is rebuilt into the shape of a v2 `session_message` row, so
//! [`super::session::SessionParser`] reads both versions.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};

use super::MessageRow;

/// Messages of the session that changed at or after `watermark` (unix ms),
/// rebuilt as v2 rows, oldest first. A message changes when it or one of its
/// parts is rewritten.
pub(super) fn rows(
    conn: &Connection,
    session_id: &str,
    watermark: i64,
) -> rusqlite::Result<Vec<MessageRow>> {
    let mut messages = conn.prepare_cached(
        "SELECT id, time_updated, data FROM message m
         WHERE session_id = ?1 AND (time_updated >= ?2 OR EXISTS (
             SELECT 1 FROM part p WHERE p.message_id = m.id AND p.time_updated >= ?2))
         ORDER BY time_created, id",
    )?;
    let changed = messages
        .query_map(params![session_id, watermark], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::with_capacity(changed.len());
    for (id, mut time_updated, data) in changed {
        let (parts, parts_updated) = parts(conn, &id)?;
        time_updated = time_updated.max(parts_updated);
        let message = serde_json::from_str(&data).unwrap_or(Value::Null);
        let (kind, data) = to_v2(&message, &parts);
        out.push(MessageRow {
            id,
            kind: kind.to_string(),
            time_updated,
            data: data.to_string(),
        });
    }
    Ok(out)
}

/// The session's newest message as a v2 row type and its `message.data`,
/// for deciding whether a turn is open.
pub(super) fn last_message(
    conn: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<(&'static str, String)>> {
    let data: Option<String> = conn
        .query_row(
            "SELECT data FROM message WHERE session_id = ?1
             ORDER BY time_created DESC, id DESC LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(data.map(|data| {
        let message: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        let kind = match role(&message) {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => "unknown",
        };
        (kind, data)
    }))
}

/// Text of the session's first prompt that the user wrote.
pub(super) fn first_prompt(conn: &Connection, session_id: &str) -> Option<String> {
    let mut stmt = conn
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id")
        .ok()?;
    let mut rows = stmt.query(params![session_id]).ok()?;
    while let Some(row) = rows.next().ok()? {
        let data: String = row.get(1).ok()?;
        let message: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        if role(&message) != Some("user") {
            continue;
        }
        let id: String = row.get(0).ok()?;
        let (parts, _) = parts(conn, &id).ok()?;
        let text = prompt_text(&parts);
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    None
}

/// A message's parts in order, and the newest part's `time_updated`.
fn parts(conn: &Connection, message_id: &str) -> rusqlite::Result<(Vec<Value>, i64)> {
    let mut stmt = conn
        .prepare_cached("SELECT time_updated, data FROM part WHERE message_id = ?1 ORDER BY id")?;
    let mut parts = Vec::new();
    let mut newest = 0;
    let mut rows = stmt.query(params![message_id])?;
    while let Some(row) = rows.next()? {
        newest = newest.max(row.get::<_, i64>(0)?);
        if let Ok(part) = serde_json::from_str(&row.get::<_, String>(1)?) {
            parts.push(part);
        }
    }
    Ok((parts, newest))
}

fn role(message: &Value) -> Option<&str> {
    message.get("role").and_then(Value::as_str)
}

fn str_field<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

fn is_true(value: &Value, name: &str) -> bool {
    value.get(name).and_then(Value::as_bool) == Some(true)
}

/// Rebuild a v1 message and its parts as a v2 row type and `data`.
fn to_v2(message: &Value, parts: &[Value]) -> (&'static str, Value) {
    let created = message.pointer("/time/created").cloned();
    match role(message) {
        Some("user") if parts.iter().any(|p| str_field(p, "type") == "compaction") => {
            ("compaction", json!({ "time": { "created": created } }))
        }
        Some("user") => (
            "user",
            json!({ "time": { "created": created }, "text": prompt_text(parts) }),
        ),
        // The summary written for a compaction is not a reply to the user.
        Some("assistant") if is_true(message, "summary") => ("summary", Value::Null),
        Some("assistant") => {
            let mut data = Map::new();
            data.insert(
                "time".into(),
                times(
                    message.pointer("/time/created"),
                    message.pointer("/time/completed"),
                ),
            );
            if let Some(finish) = message.get("finish") {
                data.insert("finish".into(), finish.clone());
            }
            if let Some(error) = message.get("error").filter(|e| !e.is_null()) {
                data.insert("error".into(), message_error(error));
            }
            data.insert(
                "content".into(),
                Value::Array(parts.iter().filter_map(content_block).collect()),
            );
            ("assistant", Value::Object(data))
        }
        _ => ("unknown", Value::Null),
    }
}

/// The text the user wrote: the prompt's text parts, without the ones
/// opencode added (such as the contents of attached files).
fn prompt_text(parts: &[Value]) -> String {
    parts
        .iter()
        .filter(|p| {
            str_field(p, "type") == "text" && !is_true(p, "synthetic") && !is_true(p, "ignored")
        })
        .map(|p| str_field(p, "text"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A reply part as a v2 `content` block, or `None` for parts the v2 parser
/// doesn't read.
fn content_block(part: &Value) -> Option<Value> {
    match str_field(part, "type") {
        "text" if !is_true(part, "synthetic") && !is_true(part, "ignored") => {
            Some(json!({ "type": "text", "text": str_field(part, "text") }))
        }
        "reasoning" => Some(json!({
            "type": "reasoning",
            "text": str_field(part, "text"),
            "time": times(part.pointer("/time/start"), part.pointer("/time/end")),
        })),
        "tool" => {
            let mut state = Map::new();
            for key in ["status", "input", "error"] {
                if let Some(value) = part.pointer(&format!("/state/{key}")) {
                    state.insert(key.into(), value.clone());
                }
            }
            Some(json!({
                "type": "tool",
                "id": str_field(part, "callID"),
                "name": str_field(part, "tool"),
                "time": times(part.pointer("/state/time/start"), part.pointer("/state/time/end")),
                "state": state,
            }))
        }
        _ => None,
    }
}

/// `{ created, completed }`, leaving out the ones that aren't set.
fn times(created: Option<&Value>, completed: Option<&Value>) -> Value {
    let mut time = Map::new();
    for (key, value) in [("created", created), ("completed", completed)] {
        if let Some(value) = value.filter(|v| !v.is_null()) {
            time.insert(key.into(), value.clone());
        }
    }
    Value::Object(time)
}

/// A v1 message error (`{ name, data: { message } }`) in the v2 shape
/// (`{ type, message }`). An abort is `MessageAbortedError` in v1 and
/// `aborted` in v2.
fn message_error(error: &Value) -> Value {
    let kind = match str_field(error, "name") {
        "MessageAbortedError" => "aborted",
        name => name,
    };
    json!({
        "type": kind,
        "message": error.pointer("/data/message").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::opencode::session::SessionParser;
    use crate::agents::{AgentEventKind, ToolOutcome};

    /// Feed v1 messages through `to_v2` and the v2 parser.
    fn parse(messages: &[(&str, Value, Vec<Value>)]) -> Vec<AgentEventKind> {
        let mut parser = SessionParser::new();
        messages
            .iter()
            .flat_map(|(id, message, parts)| {
                let (kind, data) = to_v2(message, parts);
                parser.process(id, kind, 0, &data.to_string())
            })
            .map(|e| e.kind)
            .collect()
    }

    #[test]
    fn prompt_skips_added_text() {
        let events = parse(&[(
            "u",
            json!({"role": "user", "time": {"created": 1}}),
            vec![
                json!({"type": "text", "text": "fix it"}),
                json!({"type": "text", "text": "file contents", "synthetic": true}),
                json!({"type": "file", "filename": "a.rs"}),
            ],
        )]);
        assert_eq!(
            events,
            vec![AgentEventKind::Prompt {
                text: "fix it".into(),
                pasted_chars: None,
                scripted: false,
            }]
        );
    }

    #[test]
    fn reply_with_tools() {
        let events = parse(&[
            (
                "u",
                json!({"role": "user", "time": {"created": 1}}),
                vec![json!({"type": "text", "text": "go"})],
            ),
            (
                "a",
                json!({"role": "assistant", "time": {"created": 2, "completed": 9}, "finish": "stop"}),
                vec![
                    json!({"type": "step-start"}),
                    json!({"type": "reasoning", "text": "hm", "time": {"start": 3, "end": 4}}),
                    json!({"type": "tool", "callID": "c1", "tool": "bash", "state": {
                        "status": "completed", "input": {"command": "ls"}, "time": {"start": 5, "end": 6}}}),
                    json!({"type": "tool", "callID": "c2", "tool": "edit", "state": {
                        "status": "error", "input": {},
                        "error": "Error: The user rejected permission to use this specific tool call."}}),
                    json!({"type": "text", "text": "Done"}),
                    json!({"type": "step-finish"}),
                ],
            ),
        ]);
        assert_eq!(
            events,
            vec![
                AgentEventKind::Prompt {
                    text: "go".into(),
                    pasted_chars: None,
                    scripted: false,
                },
                AgentEventKind::Thinking { text: "hm".into() },
                AgentEventKind::ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"})
                },
                AgentEventKind::ToolResult {
                    id: "c1".into(),
                    outcome: ToolOutcome::Success
                },
                AgentEventKind::ToolCall {
                    id: "c2".into(),
                    name: "edit".into(),
                    input: json!({})
                },
                AgentEventKind::ToolResult {
                    id: "c2".into(),
                    outcome: ToolOutcome::Rejected
                },
                AgentEventKind::Response {
                    text: "Done".into()
                },
                AgentEventKind::TurnEnd {
                    duration_ms: Some(8)
                },
            ]
        );
    }

    #[test]
    fn abort_is_an_interrupt() {
        let events = parse(&[(
            "a",
            json!({"role": "assistant", "time": {"created": 1, "completed": 5},
                   "error": {"name": "MessageAbortedError", "data": {"message": "Aborted"}}}),
            vec![
                json!({"type": "tool", "callID": "c", "tool": "bash", "state": {
                "status": "error", "input": {}, "error": "Tool execution aborted"}}),
            ],
        )]);
        assert_eq!(
            events.last(),
            Some(&AgentEventKind::Interrupt {
                during_tool_use: true
            })
        );
    }

    #[test]
    fn compaction_without_its_summary() {
        let events = parse(&[
            (
                "u",
                json!({"role": "user", "time": {"created": 1}}),
                vec![json!({"type": "compaction", "auto": true})],
            ),
            (
                "a",
                json!({"role": "assistant", "summary": true, "time": {"created": 2, "completed": 3}, "finish": "stop"}),
                vec![json!({"type": "text", "text": "Summary of the conversation"})],
            ),
        ]);
        assert_eq!(events, vec![AgentEventKind::Compacted]);
    }

    #[test]
    fn reads_the_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                                   time_updated INTEGER, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                                time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO message VALUES
               ('m1', 's', 1, 1, '{\"role\":\"user\",\"time\":{\"created\":1}}'),
               ('m2', 's', 2, 2, '{\"role\":\"assistant\",\"time\":{\"created\":2}}');
             INSERT INTO part VALUES
               ('p1', 'm1', 's', 1, 1, '{\"type\":\"text\",\"text\":\"hi\"}'),
               ('p2', 'm2', 's', 2, 7, '{\"type\":\"text\",\"text\":\"streaming\"}');",
        )
        .unwrap();

        let all = rows(&conn, "s", 0).unwrap();
        assert_eq!(
            all.iter()
                .map(|r| (r.kind.as_str(), r.time_updated))
                .collect::<Vec<_>>(),
            vec![("user", 1), ("assistant", 7)]
        );
        // Only the reply has a part newer than 5.
        let changed = rows(&conn, "s", 5).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].id, "m2");

        assert_eq!(first_prompt(&conn, "s").as_deref(), Some("hi"));
        assert_eq!(
            last_message(&conn, "s").unwrap().map(|m| m.0),
            Some("assistant")
        );
    }
}
