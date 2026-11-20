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
//!
//! Everything else (`attachment`, `api-request*`, `queue-operation`, ...) is
//! ignored. The format is undocumented and changes between versions, so
//! unknown lines and fields are skipped rather than treated as errors.

use serde_json::Value;

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
            Some("user") => user_events(&entry),
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

fn user_events(entry: &Value) -> Vec<AgentEventKind> {
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
                            Some(AgentEventKind::Prompt { text }) => prompt.push(text),
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
    Some(AgentEventKind::Prompt {
        text: text.to_string(),
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
    blocks
        .iter()
        .filter_map(|block| {
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
        })
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

/// Parse an RFC 3339 timestamp such as `2026-11-01T20:17:04.957Z` to unix ms.
pub fn parse_timestamp_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(r)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }

    let mut i = 19;
    let mut millis = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            if i - start < 3 {
                millis = millis * 10 + i64::from(b[i] - b'0');
            }
            i += 1;
        }
        for _ in (i - start)..3 {
            millis *= 10;
        }
    }
    let offset_min = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let m = num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?;
            if *sign == b'+' { m } else { -m }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + min * 60 + sec - offset_min * 60;
    u64::try_from(secs * 1000 + millis).ok()
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(p: &mut TranscriptParser, line: &str) -> Vec<AgentEventKind> {
        p.parse_line(line).into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_timestamp_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp_ms("2026-11-01T20:17:04.957Z"),
            Some(1_793_564_224_957)
        );
        assert_eq!(
            parse_timestamp_ms("2026-11-01T21:17:04.9+01:00"),
            Some(1_793_564_224_900)
        );
        assert_eq!(parse_timestamp_ms("2026-11-01"), None);
        assert_eq!(parse_timestamp_ms("2026-13-01T00:00:00Z"), None);
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
                    text: "Fix it".into()
                }
            }]
        );
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
