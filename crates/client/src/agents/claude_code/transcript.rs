//! Turns Claude Code transcript lines into [`AgentEvent`]s.
//!
//! Claude Code appends every session to
//! `~/.claude/projects/<escaped cwd>/<session id>.jsonl`, one JSON object per
//! line. The lines we use:
//!
//! - `user`: a prompt (string content), tool results (`tool_result`
//!   blocks), an interrupt (`[Request interrupted by user...]` text), or a
//!   slash command (`<command-name>` content). `isMeta` lines are injected
//!   context, not something the user did.
//! - `assistant`: one content block per line (`text`, `thinking`,
//!   `tool_use`).
//! - `system` with subtype `turn_duration` (turn finished) or
//!   `compact_boundary` (conversation compacted).
//! - `custom-title` / `ai-title`: the session title, repeated many times.
//! - `queue-operation` with `operation: "enqueue"`: a message the user sent
//!   while the agent was working, written the moment it was sent. It later
//!   reaches the agent either mid-turn, as an `attachment` of type
//!   `queued_command` (no `user` line), or as the next turn's `user` line;
//!   both are written only then, often seconds to minutes later. So the
//!   prompt is taken from the enqueue line and the later copy is skipped.
//!   Enqueued text starting with `<` (task notifications, sub-agents'
//!   messages) or `/` (commands) is left to the later line, whose origin
//!   says who sent it.
//! - `attachment` of type `queued_command` with `commandMode: "prompt"`
//!   from a human: a prompt, unless its enqueue line already gave it.
//!
//! Everything else (other attachments, `api-request*`, other queue
//! operations, ...) is ignored. The format is undocumented and changes between versions, so
//! unknown lines and fields are skipped rather than treated as errors.

use serde_json::Value;

use crate::agents::time::parse_timestamp_ms;
use crate::agents::{AgentEvent, AgentEventKind, ToolOutcome};

const INTERRUPT: &str = "[Request interrupted by user";
const INTERRUPT_TOOL_USE: &str = "[Request interrupted by user for tool use]";
const REJECTED: &str = "The user doesn't want to proceed with this tool use";

/// Parses one session's lines in order. Keeps the little state needed to
/// emit title changes only when the title actually changes.
#[derive(Debug, Default)]
pub struct TranscriptParser {
    title: Option<String>,
    has_custom_title: bool,
    last_time_ms: u64,
    /// Prompts taken from enqueue lines whose later copy hasn't been seen
    /// yet, by text.
    enqueued: std::collections::HashMap<String, usize>,
}

impl TranscriptParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current session title, if one has been seen.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Events in one transcript line. Malformed lines produce none.
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

        // Subagent conversations are written into the same file; they are
        // not something the user and the main agent did.
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return Vec::new();
        }

        let kinds = match entry.get("type").and_then(Value::as_str) {
            Some("queue-operation") => self.enqueued_prompt(&entry),
            Some("user") => self.skip_enqueued(user_events(&entry)),
            Some("attachment") => self.skip_enqueued(queued_prompt(&entry)),
            Some("assistant") => assistant_events(&entry),
            Some("system") => system_events(&entry),
            Some("custom-title") => self.title_events(&entry, "customTitle", true),
            Some("ai-title") => self.title_events(&entry, "aiTitle", false),
            Some("summary") => self.title_events(&entry, "summary", false),
            _ => Vec::new(),
        };
        kinds
            .into_iter()
            .map(|kind| AgentEvent { time_ms, kind })
            .collect()
    }

    /// A prompt sent while the agent was working, from its enqueue line.
    fn enqueued_prompt(&mut self, entry: &Value) -> Vec<AgentEventKind> {
        if entry.get("operation").and_then(Value::as_str) != Some("enqueue") {
            return Vec::new();
        }
        let Some(text) = entry.get("content").and_then(Value::as_str) else {
            return Vec::new();
        };
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('<') || trimmed.starts_with('/') {
            return Vec::new();
        }
        *self.enqueued.entry(trimmed.to_string()).or_default() += 1;
        vec![AgentEventKind::Prompt {
            text: text.to_string(),
            pasted_chars: None,
            scripted: false,
        }]
    }

    /// Drop prompts already taken from their enqueue line.
    fn skip_enqueued(&mut self, events: Vec<AgentEventKind>) -> Vec<AgentEventKind> {
        events
            .into_iter()
            .filter(|event| {
                let AgentEventKind::Prompt { text, .. } = event else {
                    return true;
                };
                match self.enqueued.get_mut(text.trim()) {
                    Some(n) => {
                        *n -= 1;
                        if *n == 0 {
                            self.enqueued.remove(text.trim());
                        }
                        false
                    }
                    None => true,
                }
            })
            .collect()
    }

    fn title_events(&mut self, entry: &Value, field: &str, custom: bool) -> Vec<AgentEventKind> {
        // A title set by the user wins over generated ones.
        if self.has_custom_title && !custom {
            return Vec::new();
        }
        let Some(title) = entry.get(field).and_then(Value::as_str) else {
            return Vec::new();
        };
        self.has_custom_title |= custom;
        if self.title.as_deref() == Some(title) {
            return Vec::new();
        }
        self.title = Some(title.to_string());
        vec![AgentEventKind::TitleChanged {
            title: title.to_string(),
        }]
    }
}

/// A message the user sent while the agent was working (see the module
/// docs), as a prompt.
fn queued_prompt(entry: &Value) -> Vec<AgentEventKind> {
    let Some(attachment) = entry.get("attachment") else {
        return Vec::new();
    };
    let field = |name| attachment.get(name).and_then(Value::as_str);
    if field("type") != Some("queued_command") || field("commandMode") != Some("prompt") {
        return Vec::new();
    }
    let human = match attachment.pointer("/origin/kind").and_then(Value::as_str) {
        Some(kind) => kind == "human",
        None => attachment.get("humanTurn").and_then(Value::as_bool) == Some(true),
    };
    if !human {
        return Vec::new();
    }
    // A string, or content blocks when images were attached.
    let text = match attachment.get("prompt") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return Vec::new(),
    };
    let mut events: Vec<_> = text_events(&text, true).into_iter().collect();
    mark_scripted(entry, &mut events);
    events
}

fn user_events(entry: &Value) -> Vec<AgentEventKind> {
    let mut events = user_events_of(entry);
    mark_scripted(entry, &mut events);
    events
}

/// Mark prompts of a session a program started as scripted.
fn mark_scripted(entry: &Value, events: &mut [AgentEventKind]) {
    // `entrypoint` names how the session was started: `cli`, an editor or
    // desktop app, or `sdk-cli` / `sdk-ts` / `sdk-py` for programs (the
    // Agent SDK, `claude -p`).
    if entry
        .get("entrypoint")
        .and_then(Value::as_str)
        .is_some_and(|e| e.starts_with("sdk"))
    {
        for event in events {
            if let AgentEventKind::Prompt { scripted, .. } = event {
                *scripted = true;
            }
        }
    }
}

fn user_events_of(entry: &Value) -> Vec<AgentEventKind> {
    let flag = |name| entry.get(name).and_then(Value::as_bool) == Some(true);
    if flag("isMeta") || flag("isCompactSummary") || flag("isVisibleInTranscriptOnly") {
        return Vec::new();
    }
    let Some(content) = entry.pointer("/message/content") else {
        return Vec::new();
    };

    // Newer versions say who wrote the message. Without it, fall back to
    // recognising generated messages by their markup.
    let origin = entry.pointer("/origin/kind").and_then(Value::as_str);
    let from_human = origin.is_none_or(|k| k == "human");

    match content {
        Value::String(text) => text_events(text, from_human).into_iter().collect(),
        Value::Array(blocks) => {
            let mut out = Vec::new();
            let mut prompt = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => out.push(tool_result_event(entry, block)),
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                        match text_events(text, from_human) {
                            Some(AgentEventKind::Prompt { text, .. }) => prompt.push(text),
                            Some(other) => out.push(other),
                            None => {}
                        }
                    }
                    _ => {} // images etc.
                }
            }
            // A prompt with pasted images is split into several text blocks.
            if !prompt.is_empty() {
                out.push(AgentEventKind::Prompt {
                    text: prompt.join("\n"),
                    pasted_chars: None,
                    scripted: false,
                });
            }
            out
        }
        _ => Vec::new(),
    }
}

/// What a piece of user text means: a prompt, a command, an interrupt, or
/// nothing (generated output).
fn text_events(text: &str, from_human: bool) -> Option<AgentEventKind> {
    if text.starts_with(INTERRUPT) {
        return Some(AgentEventKind::Interrupt {
            during_tool_use: text.starts_with(INTERRUPT_TOOL_USE),
        });
    }
    if let Some(name) = tag(text, "command-name") {
        return Some(AgentEventKind::Command {
            name: name.to_string(),
            args: tag(text, "command-args").unwrap_or("").to_string(),
        });
    }
    if !from_human {
        return None;
    }
    // Output of local commands, `!` shell commands, background task
    // notifications and similar all start with a tag.
    let trimmed = text.trim_start();
    if trimmed.starts_with("<local-command-")
        || trimmed.starts_with("<bash-")
        || trimmed.starts_with("<task-notification>")
        || trimmed.starts_with("<command-message>")
        || trimmed.starts_with("Caveat: ")
    {
        return None;
    }
    if trimmed.is_empty() {
        return None;
    }
    // Paste counts come from the prompt history, see `history.rs`.
    Some(AgentEventKind::Prompt {
        text: text.to_string(),
        pasted_chars: None,
        scripted: false,
    })
}

fn tool_result_event(entry: &Value, block: &Value) -> AgentEventKind {
    let id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let is_error = block.get("is_error").and_then(Value::as_bool) == Some(true);
    let rejected = entry.get("toolUseResult").and_then(Value::as_str)
        == Some("User rejected tool use")
        || content_text(block.get("content")).is_some_and(|t| t.starts_with(REJECTED));
    let outcome = match (rejected, is_error) {
        (true, _) => ToolOutcome::Rejected,
        (false, true) => ToolOutcome::Error,
        (false, false) => ToolOutcome::Success,
    };
    AgentEventKind::ToolResult { id, outcome }
}

/// First text of a tool result's content, which is a string or a list of
/// blocks.
fn content_text(content: Option<&Value>) -> Option<&str> {
    match content? {
        Value::String(s) => Some(s),
        Value::Array(blocks) => blocks
            .iter()
            .find_map(|b| b.get("text").and_then(Value::as_str)),
        _ => None,
    }
}

fn assistant_events(entry: &Value) -> Vec<AgentEventKind> {
    if entry.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    let Some(blocks) = entry.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };
    // Lines Claude Code writes itself (errors, notices) say `<synthetic>`.
    let model = entry
        .pointer("/message/model")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty() && !m.starts_with('<'))
        .map(|name| AgentEventKind::Model {
            name: name.to_string(),
        });
    model
        .into_iter()
        .chain(blocks.iter().filter_map(|block| {
            let str_field = |name| {
                block
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            match block.get("type").and_then(Value::as_str)? {
                "text" => Some(AgentEventKind::Response {
                    text: str_field("text"),
                }),
                "thinking" => Some(AgentEventKind::Thinking {
                    text: str_field("thinking"),
                }),
                "redacted_thinking" => Some(AgentEventKind::Thinking {
                    text: String::new(),
                }),
                "tool_use" | "server_tool_use" => Some(AgentEventKind::ToolCall {
                    id: str_field("id"),
                    name: str_field("name"),
                    input: block.get("input").cloned().unwrap_or(Value::Null),
                }),
                _ => None,
            }
        }))
        .collect()
}

fn system_events(entry: &Value) -> Vec<AgentEventKind> {
    match entry.get("subtype").and_then(Value::as_str) {
        Some("turn_duration") => vec![AgentEventKind::TurnEnd {
            duration_ms: entry.get("durationMs").and_then(Value::as_u64),
        }],
        Some("compact_boundary") => vec![AgentEventKind::Compacted],
        _ => Vec::new(),
    }
}

/// Contents of the first `<name>...</name>` in `text`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let len = text[start..].find(&close)?;
    Some(text[start..start + len].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(p: &mut TranscriptParser, line: &str) -> Vec<AgentEventKind> {
        p.parse_line(line).into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn prompt_and_time() {
        let mut p = TranscriptParser::new();
        let ev = p.parse_line(
            r#"{"type":"user","message":{"role":"user","content":"Fix it"},"timestamp":"1970-01-01T00:00:01Z","origin":{"kind":"human"}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent {
                time_ms: 1000,
                kind: AgentEventKind::Prompt {
                    text: "Fix it".into(),
                    pasted_chars: None,
                    scripted: false,
                }
            }]
        );
    }

    #[test]
    fn sdk_prompts_are_scripted() {
        let mut p = TranscriptParser::new();
        let line = |entrypoint: &str| {
            format!(
                r#"{{"type":"user","entrypoint":"{entrypoint}","message":{{"role":"user","content":"go on"}},"timestamp":"1970-01-01T00:00:01Z"}}"#
            )
        };
        let scripted = |kinds: Vec<AgentEventKind>| match kinds.as_slice() {
            [AgentEventKind::Prompt { scripted, .. }] => *scripted,
            other => panic!("not one prompt: {other:?}"),
        };
        assert!(scripted(kinds(&mut p, &line("sdk-cli"))));
        assert!(scripted(kinds(&mut p, &line("sdk-ts"))));
        assert!(!scripted(kinds(&mut p, &line("cli"))));
        assert!(!scripted(kinds(&mut p, &line("claude-desktop"))));
    }

    #[test]
    fn generated_user_lines_are_not_prompts() {
        let mut p = TranscriptParser::new();
        for line in [
            r#"{"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>x</local-command-caveat>"}}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"content":"This session is being continued"}}"#,
            r#"{"type":"user","message":{"content":"<local-command-stdout></local-command-stdout>"}}"#,
            r#"{"type":"user","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>"}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":"subagent prompt"}}"#,
        ] {
            assert_eq!(kinds(&mut p, line), vec![], "{line}");
        }
    }

    #[test]
    fn command() {
        let mut p = TranscriptParser::new();
        let line = r#"{"type":"user","message":{"content":"<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>"}}"#;
        assert_eq!(
            kinds(&mut p, line),
            vec![AgentEventKind::Command {
                name: "/model".into(),
                args: "opus".into()
            }]
        );
    }

    #[test]
    fn interrupts() {
        let mut p = TranscriptParser::new();
        let plain = r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        let tool = r#"{"type":"user","message":{"content":"[Request interrupted by user for tool use]\n"}}"#;
        assert_eq!(
            kinds(&mut p, plain),
            vec![AgentEventKind::Interrupt {
                during_tool_use: false
            }]
        );
        assert_eq!(
            kinds(&mut p, tool),
            vec![AgentEventKind::Interrupt {
                during_tool_use: true
            }]
        );
    }

    #[test]
    fn assistant_blocks() {
        let mut p = TranscriptParser::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Hi"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        assert_eq!(
            kinds(&mut p, line),
            vec![
                AgentEventKind::Thinking {
                    text: String::new()
                },
                AgentEventKind::Response { text: "Hi".into() },
                AgentEventKind::ToolCall {
                    id: "t1".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({"command":"ls"})
                },
            ]
        );
    }

    #[test]
    fn model_comes_before_the_blocks() {
        let mut p = TranscriptParser::new();
        let line = r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Hi"}]}}"#;
        assert_eq!(
            kinds(&mut p, line),
            vec![
                AgentEventKind::Model {
                    name: "claude-opus-5-5".into()
                },
                AgentEventKind::Response { text: "Hi".into() },
            ]
        );
        // Lines Claude Code writes itself name no real model.
        let synthetic = r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}}"#;
        assert_eq!(
            kinds(&mut p, synthetic),
            vec![AgentEventKind::Response {
                text: "No response requested.".into()
            }]
        );
    }

    #[test]
    fn messages_sent_mid_turn_are_prompts() {
        let mut p = TranscriptParser::new();
        let queued = r#"{"type":"attachment","isSidechain":false,"timestamp":"2026-11-25T14:42:27.403Z","attachment":{"type":"queued_command","prompt":"Also push the next release","commandMode":"prompt","origin":{"kind":"human"},"humanTurn":true}}"#;
        let events = p.parse_line(queued);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].time_ms, 1_795_617_747_403);
        assert_eq!(
            events[0].kind,
            AgentEventKind::Prompt {
                text: "Also push the next release".into(),
                pasted_chars: None,
                scripted: false,
            }
        );
        // Background task notifications and sub-agents' messages come the
        // same way but aren't the user's.
        let notification = r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification>done</task-notification>","commandMode":"task-notification","origin":{"kind":"task-notification"}}}"#;
        let peer = r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<agent-message>hi</agent-message>","commandMode":"prompt","origin":{"kind":"peer"}}}"#;
        let other =
            r#"{"type":"attachment","attachment":{"type":"edited_text_file","filename":"a.rs"}}"#;
        for line in [notification, peer, other] {
            assert_eq!(kinds(&mut p, line), vec![], "{line}");
        }
    }

    #[test]
    fn queued_prompts_count_when_sent_and_once() {
        let mut p = TranscriptParser::new();
        let enqueue = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-11-25T14:42:27.403Z","sessionId":"s","content":"Also push it"}"#;
        let handed = r#"{"type":"attachment","timestamp":"2026-11-25T14:42:27.403Z","attachment":{"type":"queued_command","prompt":"Also push it","commandMode":"prompt","origin":{"kind":"human"}}}"#;
        let next_turn = r#"{"type":"user","timestamp":"2026-11-25T14:45:00.000Z","origin":{"kind":"human"},"message":{"content":"Also push it"}}"#;
        let prompt = |text: &str| AgentEventKind::Prompt {
            text: text.into(),
            pasted_chars: None,
            scripted: false,
        };
        let events = p.parse_line(enqueue);
        assert_eq!(events[0].time_ms, 1_795_617_747_403);
        assert_eq!(events[0].kind, prompt("Also push it"));
        // Handed over mid-turn: already counted.
        assert_eq!(kinds(&mut p, handed), vec![]);
        // Sent once more later as its own prompt: that one counts.
        assert_eq!(kinds(&mut p, next_turn), vec![prompt("Also push it")]);

        // Queued, then sent as the next turn's prompt.
        assert_eq!(kinds(&mut p, enqueue), vec![prompt("Also push it")]);
        assert_eq!(kinds(&mut p, next_turn), vec![]);

        // Notifications, sub-agents' messages and commands are left to
        // their later line.
        for content in ["<task-notification>x</task-notification>", "/context"] {
            let line = format!(
                r#"{{"type":"queue-operation","operation":"enqueue","content":"{content}"}}"#
            );
            assert_eq!(kinds(&mut p, &line), vec![], "{content}");
        }
        let dequeue = r#"{"type":"queue-operation","operation":"dequeue"}"#;
        assert_eq!(kinds(&mut p, dequeue), vec![]);
    }

    #[test]
    fn tool_results() {
        let mut p = TranscriptParser::new();
        let ok = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":"out"}]}}"#;
        let err = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","is_error":true,"content":"boom"}]}}"#;
        let rej = r#"{"type":"user","toolUseResult":"User rejected tool use","message":{"content":[{"type":"tool_result","tool_use_id":"c","is_error":true,"content":"The user doesn't want to proceed with this tool use."}]}}"#;
        let result = |id: &str, outcome| AgentEventKind::ToolResult {
            id: id.into(),
            outcome,
        };
        assert_eq!(kinds(&mut p, ok), vec![result("a", ToolOutcome::Success)]);
        assert_eq!(kinds(&mut p, err), vec![result("b", ToolOutcome::Error)]);
        assert_eq!(kinds(&mut p, rej), vec![result("c", ToolOutcome::Rejected)]);
    }

    #[test]
    fn titles() {
        let mut p = TranscriptParser::new();
        let ai = r#"{"type":"ai-title","aiTitle":"Generated"}"#;
        let custom = r#"{"type":"custom-title","customTitle":"Mine"}"#;
        let changed = |t: &str| vec![AgentEventKind::TitleChanged { title: t.into() }];
        assert_eq!(kinds(&mut p, ai), changed("Generated"));
        assert_eq!(kinds(&mut p, ai), vec![]);
        assert_eq!(kinds(&mut p, custom), changed("Mine"));
        assert_eq!(kinds(&mut p, ai), vec![]);
        assert_eq!(p.title(), Some("Mine"));
    }

    #[test]
    fn turn_end_and_compaction() {
        let mut p = TranscriptParser::new();
        let end = r#"{"type":"system","subtype":"turn_duration","durationMs":42}"#;
        let compact = r#"{"type":"system","subtype":"compact_boundary"}"#;
        assert_eq!(
            kinds(&mut p, end),
            vec![AgentEventKind::TurnEnd {
                duration_ms: Some(42)
            }]
        );
        assert_eq!(kinds(&mut p, compact), vec![AgentEventKind::Compacted]);
    }
}
