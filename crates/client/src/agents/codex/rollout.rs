//! Turns Codex rollout lines into [`AgentEvent`]s.
//!
//! Every line of a rollout is `{timestamp, type, payload}`. Two formats
//! exist, and one file uses only one of them:
//!
//! - legacy (v0.32 to v0.150, and older sessions resumed later): prompts,
//!   replies and reasoning are `event_msg` lines of type `user_message`,
//!   `agent_message` and `agent_reasoning`.
//! - paginated (since v0.144; the default for new terminal and editor
//!   threads since v0.151): they are `event_msg` lines of type
//!   `item_completed` whose `item` is a `UserMessage`, `AgentMessage` or
//!   `Reasoning`.
//!
//! Turn starts and ends (`task_started`, `task_complete`) are only written
//! since v0.100; `turn_aborted` since v0.35. Files from before v0.32 have no
//! `{timestamp, type, payload}` wrapper and produce no events.
//!
//! A legacy fork (v0.81 to v0.150) copies its parent's whole history into
//! the new file, parent `session_meta` included, with fork-time timestamps.
//! That copied block is skipped so nothing is counted twice: it starts at a
//! second `session_meta` line and is written in one burst, so it ends at
//! the first line written more than [`COPY_GAP_MS`] after the line before.
//! (Paginated forks reference the parent instead of copying it.) Sources
//! for all of this are in Codex's repository; see the integration's spec.
//!
//! Their record types don't overlap, so both are parsed without knowing
//! which one a file uses. Both have `event_msg` `task_complete` (a turn
//! ended), a top-level `compacted` line, and a `turn_context` line at the
//! start of each turn naming the `model`.
//!
//! Tool calls are `response_item` lines in both formats: `function_call`,
//! `custom_tool_call`, `local_shell_call` and `web_search_call`, written
//! when the model issues the call, and `function_call_output` /
//! `custom_tool_call_output` when it finishes. The paginated format also
//! writes an `item_completed` (`CommandExecution`, `FileChange`,
//! `McpToolCall`) with the call's id and a status just before the output;
//! without one, the outcome is read from the output text.
//!
//! Injected context (environment, instructions) arrives as `response_item`
//! messages, which are not prompts. The format is undocumented, so unknown
//! lines and fields are skipped rather than treated as errors.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::agents::time::parse_timestamp_ms;
use crate::agents::{AgentEvent, AgentEventKind, ToolOutcome};

/// Lines of a copied fork history are written back to back; the first
/// line more than this after the previous one is new activity.
const COPY_GAP_MS: u64 = 1000;

/// Parses one session's lines in order.
#[derive(Debug, Default)]
pub struct RolloutParser {
    last_time_ms: u64,
    /// `session_meta` lines seen so far.
    metas: u32,
    /// While skipping a legacy fork's copied history: the time of the last
    /// copied line.
    copying_since: Option<u64>,
    /// Outcomes from `item_completed` statuses, waiting for their call's
    /// output line.
    outcomes: HashMap<String, ToolOutcome>,
    /// Tool calls of this turn without an output yet.
    open_calls: HashSet<String>,
    /// A tool output this turn said the user aborted it.
    tool_aborted: bool,
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
        let line_time = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_timestamp_ms);
        if let Some(t) = line_time {
            self.last_time_ms = t;
        }
        if entry.get("type").and_then(Value::as_str) == Some("session_meta") {
            self.metas += 1;
            if self.metas > 1 {
                // A legacy fork's copy of its parent starts here.
                self.copying_since = Some(self.last_time_ms);
            }
            return Vec::new();
        }
        if let Some(last) = self.copying_since {
            match line_time {
                Some(t) if t > last + COPY_GAP_MS => self.copying_since = None,
                _ => {
                    self.copying_since = Some(line_time.unwrap_or(last).max(last));
                    return Vec::new();
                }
            }
        }
        let time_ms = self.last_time_ms;
        let payload = entry.get("payload").unwrap_or(&Value::Null);
        let kinds = match entry.get("type").and_then(Value::as_str) {
            Some("event_msg") => self.event_msg(payload),
            Some("response_item") => self.response_item(payload),
            // The paginated `ContextCompaction` item and the legacy
            // `context_compacted` event repeat this; only this line counts.
            Some("compacted") => vec![AgentEventKind::Compacted],
            // Written at the start of every turn.
            Some("turn_context") => payload
                .get("model")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(|name| AgentEventKind::Model {
                    name: name.to_string(),
                })
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        for kind in &kinds {
            match kind {
                AgentEventKind::ToolCall { id, name, .. }
                    if !id.is_empty() && name != "web_search" =>
                {
                    self.open_calls.insert(id.clone());
                }
                AgentEventKind::ToolResult { id, .. } => {
                    self.open_calls.remove(id);
                }
                _ => {}
            }
        }
        kinds
            .into_iter()
            .map(|kind| AgentEvent { time_ms, kind })
            .collect()
    }
}

impl RolloutParser {
    fn event_msg(&mut self, payload: &Value) -> Vec<AgentEventKind> {
        let text = |name| payload.get(name).and_then(Value::as_str).unwrap_or("");
        match payload.get("type").and_then(Value::as_str) {
            Some("task_started" | "turn_started") => {
                self.end_turn();
                Vec::new()
            }
            Some("task_complete" | "turn_complete") => {
                self.end_turn();
                vec![AgentEventKind::TurnEnd {
                    duration_ms: payload.get("duration_ms").and_then(Value::as_u64),
                }]
            }
            // Esc, or cancelling an approval ("No, and tell Codex what to
            // do differently"), aborts the turn with `interrupted`. Other
            // reasons (replaced, review ended, budget reached) just end it.
            Some("turn_aborted") => {
                let during_tool_use = self.tool_aborted || !self.open_calls.is_empty();
                self.end_turn();
                if text("reason") == "interrupted" {
                    vec![AgentEventKind::Interrupt { during_tool_use }]
                } else {
                    vec![AgentEventKind::TurnEnd {
                        duration_ms: payload.get("duration_ms").and_then(Value::as_u64),
                    }]
                }
            }
            Some("user_message") => prompt(text("message")),
            Some("agent_message") => response(text("message")),
            Some("agent_reasoning") => vec![AgentEventKind::Thinking {
                text: text("text").to_string(),
            }],
            Some("item_completed") => self.item(payload.get("item").unwrap_or(&Value::Null)),
            _ => Vec::new(),
        }
    }

    /// A completed item of the paginated format.
    fn item(&mut self, item: &Value) -> Vec<AgentEventKind> {
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
            // A command the user ran with `!` in the prompt. It has no tool
            // call or output line.
            Some("CommandExecution")
                if item.get("source").and_then(Value::as_str) == Some("user_shell") =>
            {
                vec![AgentEventKind::Command {
                    name: "!".to_string(),
                    args: shell_command(item.get("command")),
                }]
            }
            // The call's output line follows and becomes the result.
            Some("CommandExecution" | "FileChange" | "McpToolCall") => {
                let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                let outcome = match item.get("status").and_then(Value::as_str) {
                    Some("completed") => Some(ToolOutcome::Success),
                    Some("failed") => Some(ToolOutcome::Error),
                    Some("declined") => Some(ToolOutcome::Rejected),
                    _ => None,
                };
                if let (false, Some(outcome)) = (id.is_empty(), outcome) {
                    self.outcomes.insert(id.to_string(), outcome);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn end_turn(&mut self) {
        self.open_calls.clear();
        self.tool_aborted = false;
    }

    fn response_item(&mut self, payload: &Value) -> Vec<AgentEventKind> {
        let str_field = |name| payload.get(name).and_then(Value::as_str).unwrap_or("");
        let call = |name: &str, input: Value| {
            vec![AgentEventKind::ToolCall {
                id: str_field("call_id").to_string(),
                name: name.to_string(),
                input,
            }]
        };
        match payload.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                // The arguments are a JSON document inside a string.
                let args = str_field("arguments");
                let input =
                    serde_json::from_str(args).unwrap_or_else(|_| Value::String(args.to_string()));
                call(str_field("name"), input)
            }
            Some("custom_tool_call") => call(
                str_field("name"),
                payload.get("input").cloned().unwrap_or(Value::Null),
            ),
            Some("local_shell_call") => call(
                "local_shell",
                payload.get("action").cloned().unwrap_or(Value::Null),
            ),
            // Runs on the provider's side: no output line follows.
            Some("web_search_call") => call(
                "web_search",
                payload.get("action").cloned().unwrap_or(Value::Null),
            ),
            Some("function_call_output" | "custom_tool_call_output") => {
                let id = str_field("call_id").to_string();
                let text = output_text(payload.get("output"));
                // Codex closes tools the user stopped with this output
                // before it writes `turn_aborted`.
                if text.starts_with("aborted by user") || text.contains("\naborted by user") {
                    self.tool_aborted = true;
                }
                let outcome = self
                    .outcomes
                    .remove(&id)
                    .unwrap_or_else(|| outcome_of(&text));
                vec![AgentEventKind::ToolResult { id, outcome }]
            }
            _ => Vec::new(),
        }
    }
}

/// A command's text: the last element of an argv such as
/// `["/bin/zsh", "-lc", "git log"]`, or the string itself.
fn shell_command(command: Option<&Value>) -> String {
    match command {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(argv)) => argv
            .last()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// A tool output as text. It is a string, a list of content items, or
/// (older versions) an object with `content` or `output`.
fn output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(_)) => joined(output, "text"),
        Some(Value::Object(o)) => o
            .get("content")
            .or_else(|| o.get("output"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// A tool call's outcome from its output text, for outputs without a
/// status: the legacy format, and tools other than commands and edits.
fn outcome_of(output: &str) -> ToolOutcome {
    if output.contains("rejected by user") {
        return ToolOutcome::Rejected;
    }
    let failed = output.starts_with("aborted by user")
        || output.contains("\naborted by user")
        || output.starts_with("apply_patch verification failed")
        || output.starts_with("exec_command failed")
        || exit_code(output).is_some_and(|code| code != 0);
    if failed {
        ToolOutcome::Error
    } else {
        ToolOutcome::Success
    }
}

/// The exit code a shell tool reported, if any: `Exit code: N` (shell
/// tool), `Process exited with code N` (unified exec), or a JSON document
/// with `metadata.exit_code` (older versions).
fn exit_code(output: &str) -> Option<i64> {
    if output.starts_with('{')
        && let Ok(doc) = serde_json::from_str::<Value>(output)
    {
        return doc.pointer("/metadata/exit_code").and_then(Value::as_i64);
    }
    // The code is in the header lines before the command's own output.
    output.lines().take(6).find_map(|line| {
        line.strip_prefix("Exit code: ")
            .or_else(|| line.strip_prefix("Process exited with code "))?
            .trim()
            .parse()
            .ok()
    })
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
        scripted: false,
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
            scripted: false,
        }
    }

    #[test]
    fn turn_context_names_the_model() {
        let mut p = RolloutParser::new();
        let line = r#"{"timestamp":"2026-02-26T20:19:08.007Z","type":"turn_context","payload":{"turn_id":"t","cwd":"/p","approval_policy":"on-request","model":"gpt-5.5-codex"}}"#;
        assert_eq!(
            kinds(&mut p, line),
            vec![AgentEventKind::Model {
                name: "gpt-5.5-codex".into()
            }]
        );
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

    fn result(id: &str, outcome: ToolOutcome) -> Vec<AgentEventKind> {
        vec![AgentEventKind::ToolResult {
            id: id.into(),
            outcome,
        }]
    }

    #[test]
    fn tool_calls() {
        let mut p = RolloutParser::new();
        let function = r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls\"}","call_id":"c1"}}"#;
        let bad_args = r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"not json","call_id":"c2"}}"#;
        let custom = r#"{"type":"response_item","payload":{"type":"custom_tool_call","status":"completed","call_id":"c3","name":"apply_patch","input":"*** Begin Patch"}}"#;
        let local = r#"{"type":"response_item","payload":{"type":"local_shell_call","call_id":"c4","status":"completed","action":{"type":"exec","command":["ls"]}}}"#;
        let search = r#"{"type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"rust"}}}"#;
        let call = |id: &str, name: &str, input: Value| {
            vec![AgentEventKind::ToolCall {
                id: id.into(),
                name: name.into(),
                input,
            }]
        };
        assert_eq!(
            kinds(&mut p, function),
            call("c1", "exec_command", serde_json::json!({"cmd": "ls"}))
        );
        assert_eq!(
            kinds(&mut p, bad_args),
            call("c2", "shell", "not json".into())
        );
        assert_eq!(
            kinds(&mut p, custom),
            call("c3", "apply_patch", "*** Begin Patch".into())
        );
        assert_eq!(
            kinds(&mut p, local),
            call(
                "c4",
                "local_shell",
                serde_json::json!({"type": "exec", "command": ["ls"]})
            )
        );
        assert_eq!(
            kinds(&mut p, search),
            call(
                "",
                "web_search",
                serde_json::json!({"type": "search", "query": "rust"})
            )
        );
    }

    #[test]
    fn outcome_from_item_status() {
        let mut p = RolloutParser::new();
        let output = |id: &str| {
            format!(
                r#"{{"type":"response_item","payload":{{"type":"function_call_output","call_id":"{id}","output":"Process exited with code 0"}}}}"#
            )
        };
        for (id, kind, status, expected) in [
            ("a", "CommandExecution", "failed", ToolOutcome::Error),
            ("b", "CommandExecution", "declined", ToolOutcome::Rejected),
            ("c", "FileChange", "completed", ToolOutcome::Success),
            ("d", "McpToolCall", "failed", ToolOutcome::Error),
        ] {
            let item = format!(
                r#"{{"type":"event_msg","payload":{{"type":"item_completed","item":{{"type":"{kind}","id":"{id}","status":"{status}"}}}}}}"#
            );
            assert_eq!(kinds(&mut p, &item), vec![]);
            assert_eq!(
                kinds(&mut p, &output(id)),
                result(id, expected),
                "{kind} {status}"
            );
        }
        // A status is used once; a later output with the same id reads its text.
        assert_eq!(
            kinds(&mut p, &output("a")),
            result("a", ToolOutcome::Success)
        );
    }

    #[test]
    fn outcome_from_output_text() {
        let cases = [
            (
                "Exit code: 0\nWall time: 0.1 seconds\nOutput:\nok",
                ToolOutcome::Success,
            ),
            (
                "Exit code: 2\nWall time: 0.1 seconds\nOutput:\n",
                ToolOutcome::Error,
            ),
            (
                "Chunk ID: ab\nWall time: 0.0003 seconds\nProcess exited with code 1\nOutput:\n",
                ToolOutcome::Error,
            ),
            (
                r#"{"output":"Success.","metadata":{"exit_code":0,"duration_seconds":0.1}}"#,
                ToolOutcome::Success,
            ),
            (
                r#"{"output":"boom","metadata":{"exit_code":1,"duration_seconds":0.1}}"#,
                ToolOutcome::Error,
            ),
            ("exec command rejected by user", ToolOutcome::Rejected),
            ("patch rejected by user", ToolOutcome::Rejected),
            (
                "apply_patch verification failed: no match",
                ToolOutcome::Error,
            ),
            (
                "exec_command failed: CreateProcess { message: \"denied\" }",
                ToolOutcome::Error,
            ),
            ("aborted by user after 11.4s", ToolOutcome::Error),
            (
                "Wall time: 3.0 seconds\naborted by user",
                ToolOutcome::Error,
            ),
            ("Plan updated", ToolOutcome::Success),
            ("", ToolOutcome::Success),
        ];
        for (output, expected) in cases {
            assert_eq!(outcome_of(output), expected, "{output}");
        }
        // Output that is a list of content items or an object.
        let list =
            serde_json::json!([{"type": "input_text", "text": "exec command rejected by user"}]);
        assert_eq!(outcome_of(&output_text(Some(&list))), ToolOutcome::Rejected);
        let object = serde_json::json!({"content": "Exit code: 1", "success": false});
        assert_eq!(outcome_of(&output_text(Some(&object))), ToolOutcome::Error);
    }

    #[test]
    fn interrupts() {
        let mut p = RolloutParser::new();
        let started = r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}"#;
        let aborted = r#"{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t","reason":"interrupted","duration_ms":9}}"#;
        let call = r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"c"}}"#;
        let stopped = r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"aborted by user after 3.0s"}}"#;
        let done = r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"Exit code: 0"}}"#;
        let interrupt = |during_tool_use| vec![AgentEventKind::Interrupt { during_tool_use }];

        // Esc while the model writes.
        kinds(&mut p, started);
        assert_eq!(kinds(&mut p, aborted), interrupt(false));

        // Esc while a command runs: the call is closed as aborted first.
        kinds(&mut p, started);
        kinds(&mut p, call);
        assert_eq!(kinds(&mut p, stopped), result("c", ToolOutcome::Error));
        assert_eq!(kinds(&mut p, aborted), interrupt(true));

        // Cancelling an approval: the call never gets an output.
        kinds(&mut p, started);
        kinds(&mut p, call);
        assert_eq!(kinds(&mut p, aborted), interrupt(true));

        // A finished tool doesn't make a later interrupt "during tool use".
        kinds(&mut p, started);
        kinds(&mut p, call);
        kinds(&mut p, done);
        assert_eq!(kinds(&mut p, aborted), interrupt(false));

        // A call left open in an earlier turn doesn't leak into this one.
        kinds(&mut p, call);
        kinds(&mut p, started);
        assert_eq!(kinds(&mut p, aborted), interrupt(false));
    }

    #[test]
    fn other_abort_reasons_end_the_turn() {
        let mut p = RolloutParser::new();
        for reason in [
            "replaced",
            "review_ended",
            "budget_limited",
            "something_new",
        ] {
            let line = format!(
                r#"{{"type":"event_msg","payload":{{"type":"turn_aborted","reason":"{reason}"}}}}"#
            );
            assert_eq!(
                kinds(&mut p, &line),
                vec![AgentEventKind::TurnEnd { duration_ms: None }],
                "{reason}"
            );
        }
    }

    #[test]
    fn user_shell_command() {
        let mut p = RolloutParser::new();
        let line = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"x","command":["/bin/zsh","-lc","git log --oneline"],"source":"user_shell","status":"failed","exit_code":128}}}"#;
        assert_eq!(
            kinds(&mut p, line),
            vec![AgentEventKind::Command {
                name: "!".into(),
                args: "git log --oneline".into()
            }]
        );
    }

    #[test]
    fn legacy_fork_copy_is_skipped() {
        let mut p = RolloutParser::new();
        let meta = |time: &str, id: &str| {
            format!(
                r#"{{"timestamp":"{time}","type":"session_meta","payload":{{"id":"{id}","cwd":"/p"}}}}"#
            )
        };
        let prompt = |time: &str, text: &str| {
            format!(
                r#"{{"timestamp":"{time}","type":"event_msg","payload":{{"type":"user_message","message":"{text}"}}}}"#
            )
        };
        let lines = [
            meta("2026-11-21T10:00:00.000Z", "fork"),
            // The parent's meta and history, rewritten at fork time.
            meta("2026-11-21T10:00:00.010Z", "parent"),
            prompt("2026-11-21T10:00:00.020Z", "parent prompt one"),
            r#"{"timestamp":"2026-11-21T10:00:00.030Z","type":"event_msg","payload":{"type":"task_complete"}}"#.to_string(),
            prompt("2026-11-21T10:00:00.900Z", "parent prompt two"),
            // The user types in the fork a few seconds later.
            prompt("2026-11-21T10:00:07.000Z", "new prompt"),
            prompt("2026-11-21T10:00:07.100Z", "steering right after"),
        ];
        let kinds: Vec<_> = lines.iter().flat_map(|l| kinds(&mut p, l)).collect();
        assert_eq!(
            kinds,
            vec![
                prompt_kind("new prompt"),
                prompt_kind("steering right after")
            ]
        );
    }

    #[test]
    fn a_single_meta_skips_nothing() {
        let mut p = RolloutParser::new();
        kinds(
            &mut p,
            r#"{"timestamp":"2026-11-21T10:00:00Z","type":"session_meta","payload":{"id":"t","cwd":"/p"}}"#,
        );
        assert_eq!(
            kinds(
                &mut p,
                r#"{"timestamp":"2026-11-21T10:00:00.100Z","type":"event_msg","payload":{"type":"user_message","message":"quick"}}"#
            ),
            vec![prompt_kind("quick")]
        );
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
