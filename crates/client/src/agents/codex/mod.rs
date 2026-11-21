//! Codex integration.
//!
//! Reads the session files the Codex CLI keeps under
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread id>.jsonl`
//! (`CODEX_HOME` defaults to `~/.codex`). Nothing is installed into Codex.
//! Each file's first line is `session_meta`, naming the thread, its working
//! directory and where it came from.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::agents::{
    AgentError, AgentEvent, AgentEventKind, AgentInstance, AgentIntegration, AttachMode,
    Attachment, is_running, shorten, tail,
};

pub mod rollout;
pub mod titles;

use rollout::RolloutParser;
use titles::{Title, Titles};

/// Bytes read from the start of a rollout to find its first prompt, for
/// sessions without a name.
const FIRST_PROMPT_SCAN: u64 = 256 * 1024;

pub struct AgentIntegrationCodex;

impl AgentIntegrationCodex {
    /// Codex's home folder: `$CODEX_HOME`, else `~/.codex`.
    pub fn home() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(PathBuf::from(home).join(".codex"))
    }

    fn installed_home() -> Option<PathBuf> {
        Self::home().filter(|h| h.join("sessions").is_dir())
    }

    /// Look up one session by its thread id.
    pub fn get_instance(&self, thread_id: &str) -> Result<AgentInstanceCodex, AgentError> {
        let home = Self::installed_home().ok_or(AgentError::NotInstalled)?;
        find_session(&home, thread_id)
            .ok_or_else(|| AgentError::SessionNotFound(thread_id.to_string()))
    }
}

impl AgentIntegration for AgentIntegrationCodex {
    fn get_name(&self) -> String {
        "Codex".to_string()
    }

    /// The user's own sessions, most recently active first.
    fn get_instances(&self) -> Option<Vec<impl AgentInstance>> {
        Self::installed_home().map(|home| list_sessions(&home))
    }

    fn install(&self) -> Result<(), String> {
        // Session files are read directly, so there is nothing to install.
        if self.is_installed() {
            Ok(())
        } else {
            Err("Codex is not installed".to_string())
        }
    }

    fn is_installed(&self) -> bool {
        Self::installed_home().is_some()
    }
}

/// One Codex session (thread), backed by its rollout file.
#[derive(Debug, Clone)]
pub struct AgentInstanceCodex {
    thread_id: String,
    title: String,
    /// The name from the session index, if the thread has one.
    named: Option<Title>,
    project_path: PathBuf,
    rollout_path: PathBuf,
    last_activity_ms: Option<u64>,
    running: bool,
}

impl AgentInstanceCodex {
    /// Read a session from its rollout file. `None` if it isn't a rollout,
    /// or isn't a thread the user drove (see [`Meta::is_user_thread`]).
    fn load(rollout_path: &Path, titles: &Titles) -> Option<Self> {
        let meta = Meta::read(rollout_path)?;
        if !meta.is_user_thread() {
            return None;
        }
        let named = titles.get(&meta.id).cloned();
        let title = match &named {
            Some(title) => title.name.clone(),
            None => first_prompt(rollout_path)
                .map(|p| shorten(&p, 60))
                .unwrap_or_else(|| "Untitled session".to_string()),
        };
        Some(Self {
            thread_id: meta.id,
            title,
            named,
            project_path: meta.cwd,
            rollout_path: rollout_path.to_path_buf(),
            running: is_running(turn_open(rollout_path), modified_ms(rollout_path)),
            last_activity_ms: modified_ms(rollout_path),
        })
    }

    pub fn rollout_path(&self) -> &Path {
        &self.rollout_path
    }

    fn ensure_exists(&self) -> Result<(), AgentError> {
        if self.rollout_path.is_file() {
            Ok(())
        } else {
            Err(AgentError::SessionNotFound(self.thread_id.clone()))
        }
    }
}

impl AgentInstance for AgentInstanceCodex {
    fn get_title(&self) -> String {
        self.title.clone()
    }

    fn get_uuid(&self) -> String {
        self.thread_id.clone()
    }

    fn get_project_path(&self) -> Option<PathBuf> {
        Some(self.project_path.clone())
    }

    fn get_last_activity_ms(&self) -> Option<u64> {
        self.last_activity_ms
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn read_events(&self) -> Result<Vec<AgentEvent>, AgentError> {
        self.ensure_exists()?;
        let mut parser = RolloutParser::new();
        let mut events = Vec::new();
        // Split on bytes so one line with invalid UTF-8 doesn't stop the
        // rest from being read.
        for line in BufReader::new(File::open(&self.rollout_path)?).split(b'\n') {
            events.extend(parser.parse_line(&String::from_utf8_lossy(&line?)));
        }
        // The name isn't in the rollout; put it where it was set.
        if let Some(title) = &self.named {
            let time_ms = title
                .updated_ms
                .or_else(|| events.first().map(|e| e.time_ms))
                .unwrap_or(0);
            let at = events.partition_point(|e| e.time_ms <= time_ms);
            events.insert(
                at,
                AgentEvent {
                    time_ms,
                    kind: AgentEventKind::TitleChanged {
                        title: title.name.clone(),
                    },
                },
            );
        }
        Ok(events)
    }

    fn attach(&self, mode: AttachMode) -> Result<Attachment, AgentError> {
        self.ensure_exists()?;
        let path = self.rollout_path.clone();
        let title = self.named.clone();
        Ok(Attachment::spawn(move |tx, stop| {
            // The name isn't in the rollout; send it first if asked for.
            if let Some(title) = title {
                let event = AgentEvent {
                    time_ms: title.updated_ms.unwrap_or(0),
                    kind: AgentEventKind::TitleChanged { title: title.name },
                };
                if mode.sends_history(&event) && tx.send(event).is_err() {
                    return;
                }
            }
            // History is always parsed so the parser knows which tool
            // calls are open, but only sent when asked for.
            let mut parser = RolloutParser::new();
            let result = tail::follow_lines(&path, &stop, |line, is_history| {
                let mut events = parser.parse_line(line);
                if is_history {
                    events.retain(|e| mode.sends_history(e));
                }
                events.into_iter().all(|e| tx.send(e).is_ok())
            });
            if let Err(e) = result {
                // The receiver only sees the channel close; there is no
                // error event, so leave a trace for debugging.
                if !stop.load(Ordering::Relaxed) {
                    eprintln!("codex: stopped following {}: {e}", path.display());
                }
            }
        }))
    }
}

/// What a rollout's first line (`session_meta`) says about its thread.
#[derive(Debug)]
struct Meta {
    id: String,
    cwd: PathBuf,
    /// `cli`, `vscode`, `exec`, `mcp`, …; `None` for structured sources
    /// (sub-agents, internal threads).
    source: Option<String>,
    /// `user`, `subagent`, `tui_side_chat`, …; absent in older versions.
    thread_source: Option<String>,
}

impl Meta {
    fn read(path: &Path) -> Option<Self> {
        let mut first = String::new();
        BufReader::new(File::open(path).ok()?)
            .read_line(&mut first)
            .ok()?;
        let line: Value = serde_json::from_str(&first).ok()?;
        if line.get("type").and_then(Value::as_str) != Some("session_meta") {
            return None;
        }
        let payload = line.get("payload")?;
        let str_field = |name| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Some(Self {
            id: str_field("id")?,
            cwd: PathBuf::from(str_field("cwd")?),
            source: str_field("source"),
            thread_source: str_field("thread_source"),
        })
    }

    /// A thread the user drove from the terminal UI or an editor, not a
    /// sub-agent, side chat, `codex exec` run or internal thread.
    fn is_user_thread(&self) -> bool {
        self.thread_source.as_deref().is_none_or(|s| s == "user")
            && matches!(self.source.as_deref(), Some("cli" | "vscode"))
    }
}

/// The user's sessions under `home`, most recently active first, one per
/// thread (a reverted thread has several files; the newest wins).
fn list_sessions(home: &Path) -> Vec<AgentInstanceCodex> {
    let titles = Titles::load(&index_path(home));
    let mut instances: Vec<AgentInstanceCodex> = rollout_files(&home.join("sessions"))
        .iter()
        .filter_map(|path| AgentInstanceCodex::load(path, &titles))
        .collect();
    instances.sort_by_key(|i| std::cmp::Reverse(i.last_activity_ms));
    let mut seen = std::collections::HashSet::new();
    instances.retain(|i| seen.insert(i.thread_id.clone()));
    instances
}

fn find_session(home: &Path, thread_id: &str) -> Option<AgentInstanceCodex> {
    let titles = Titles::load(&index_path(home));
    rollout_files(&home.join("sessions"))
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(thread_id))
        })
        .filter_map(|p| AgentInstanceCodex::load(p, &titles))
        .filter(|i| i.thread_id == thread_id)
        .max_by_key(|i| i.last_activity_ms)
}

/// Every `rollout-*.jsonl` in the dated folders `sessions/YYYY/MM/DD/`.
/// Compressed `.jsonl.zst` files are skipped.
fn rollout_files(sessions: &Path) -> Vec<PathBuf> {
    let subdirs = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
    };
    let mut files = Vec::new();
    for year in subdirs(sessions) {
        for month in subdirs(&year) {
            for day in subdirs(&month) {
                files.extend(
                    fs::read_dir(&day)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| {
                            p.extension().is_some_and(|e| e == "jsonl")
                                && p.file_name()
                                    .and_then(|n| n.to_str())
                                    .is_some_and(|n| n.starts_with("rollout-"))
                        }),
                );
            }
        }
    }
    files
}

fn index_path(home: &Path) -> PathBuf {
    home.join("session_index.jsonl")
}

/// The first prompt within the first [`FIRST_PROMPT_SCAN`] bytes.
fn first_prompt(rollout_path: &Path) -> Option<String> {
    let file = File::open(rollout_path).ok()?;
    let mut parser = RolloutParser::new();
    BufReader::new(file.take(FIRST_PROMPT_SCAN))
        .split(b'\n')
        .map_while(Result::ok)
        .flat_map(|line| parser.parse_line(&String::from_utf8_lossy(&line)))
        .find_map(|e| match e.kind {
            AgentEventKind::Prompt { text, .. } => Some(text),
            _ => None,
        })
}

/// Whether the newest turn marker in the rollout is a turn start, so the
/// agent is still working (or crashed mid-turn; see [`is_running`]).
/// Reads only the end of the file, widening the window when its lines are
/// long (a compaction can be one huge line).
fn turn_open(rollout_path: &Path) -> bool {
    let Ok(mut file) = File::open(rollout_path) else {
        return false;
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return false;
    };
    let mut window: u64 = 64 * 1024;
    loop {
        let start = len.saturating_sub(window);
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(start)).is_err()
            || (&mut file).take(window).read_to_end(&mut bytes).is_err()
        {
            return false;
        }
        let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
        if start > 0 {
            lines.remove(0); // starts mid-line
        }
        if let Some(open) = lines.iter().rev().find_map(|line| turn_marker(line)) {
            return open;
        }
        if start == 0 || window >= TURN_SCAN_MAX {
            return false;
        }
        window *= 4;
    }
}

/// Bytes read from the end of a rollout at most, looking for a turn marker.
const TURN_SCAN_MAX: u64 = 16 * 1024 * 1024;

/// `Some(true)` for a turn start, `Some(false)` for a turn end or abort.
fn turn_marker(line: &[u8]) -> Option<bool> {
    // Cheap check first; most lines aren't markers.
    let text = std::str::from_utf8(line).ok()?;
    if !text.contains("\"event_msg\"") {
        return None;
    }
    let entry: Value = serde_json::from_str(text).ok()?;
    match entry.pointer("/payload/type").and_then(Value::as_str)? {
        "task_started" | "turn_started" => Some(true),
        "task_complete" | "turn_complete" | "turn_aborted" => Some(false),
        _ => None,
    }
}

fn modified_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh, empty Codex home for one test.
    pub(crate) fn temp_home(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sisyphus-codex-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sessions/2026/11/21")).unwrap();
        dir
    }

    pub(crate) fn meta_line(
        id: &str,
        cwd: &str,
        source: &str,
        thread_source: Option<&str>,
    ) -> String {
        let thread_source = thread_source
            .map(|t| format!(r#","thread_source":"{t}""#))
            .unwrap_or_default();
        format!(
            r#"{{"timestamp":"2026-11-21T01:00:00.000Z","type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}","source":{source}{thread_source},"cli_version":"0.171.0"}}}}"#
        )
    }

    /// Write a rollout for `id` with the given lines; returns its path.
    pub(crate) fn write_rollout(home: &Path, time: &str, id: &str, lines: &[String]) -> PathBuf {
        let path = home
            .join("sessions/2026/11/21")
            .join(format!("rollout-2026-11-21T{time}-{id}.jsonl"));
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    fn ids(instances: &[AgentInstanceCodex]) -> Vec<String> {
        let mut ids: Vec<String> = instances.iter().map(|i| i.get_uuid()).collect();
        ids.sort();
        ids
    }

    #[test]
    fn lists_user_threads_only() {
        let home = temp_home("list");
        let cli = r#""cli""#;
        let vscode = r#""vscode""#;
        for (id, source, thread_source) in [
            ("user-cli", cli, Some("user")),
            ("user-vscode", vscode, Some("user")),
            ("legacy", vscode, None),
            ("exec", r#""exec""#, None),
            ("side", vscode, Some("tui_side_chat")),
            ("sub", r#"{"subagent":"review"}"#, Some("subagent")),
        ] {
            write_rollout(
                &home,
                "01-00-00",
                id,
                &[meta_line(id, "/p/todo", source, thread_source)],
            );
        }
        assert_eq!(
            ids(&list_sessions(&home)),
            ["legacy", "user-cli", "user-vscode"]
        );
        let one = list_sessions(&home)
            .into_iter()
            .find(|i| i.get_uuid() == "user-cli")
            .unwrap();
        assert_eq!(one.get_project_path(), Some(PathBuf::from("/p/todo")));
        assert!(one.get_last_activity_ms().is_some());
    }

    #[test]
    fn skips_files_that_are_not_rollouts() {
        let home = temp_home("junk");
        let day = home.join("sessions/2026/11/21");
        fs::write(
            day.join("rollout-2026-11-21T01-00-00-zst.jsonl.zst"),
            "compressed",
        )
        .unwrap();
        fs::write(day.join("rollout-2026-11-21T01-00-01-empty.jsonl"), "").unwrap();
        fs::write(
            day.join("rollout-2026-11-21T01-00-02-text.jsonl"),
            "not json\n",
        )
        .unwrap();
        fs::write(
            day.join("notes.jsonl"),
            meta_line("x", "/p", r#""cli""#, None),
        )
        .unwrap();
        assert!(list_sessions(&home).is_empty());
    }

    #[test]
    fn one_instance_per_thread_newest_file_wins() {
        let home = temp_home("dedupe");
        let meta = meta_line("t1", "/p/a", r#""cli""#, Some("user"));
        let old = write_rollout(&home, "01-00-00", "t1", std::slice::from_ref(&meta));
        let newer = write_rollout(&home, "02-00-00", "t1_r2", &[meta]);
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let listed = list_sessions(&home);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].rollout_path(), newer);
        assert_eq!(find_session(&home, "t1").unwrap().rollout_path(), newer);
        assert!(find_session(&home, "missing").is_none());
    }

    fn prompt_line(text: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-11-21T01:00:01Z","type":"event_msg","payload":{{"type":"item_completed","item":{{"type":"UserMessage","id":"u","content":[{{"type":"text","text":"{text}"}}]}}}}}}"#
        )
    }

    #[test]
    fn titles_from_index_then_first_prompt() {
        let home = temp_home("titles");
        let meta = |id| meta_line(id, "/p", r#""cli""#, Some("user"));
        write_rollout(
            &home,
            "01-00-00",
            "named",
            &[meta("named"), prompt_line("ignored")],
        );
        write_rollout(
            &home,
            "01-00-01",
            "unnamed",
            &[meta("unnamed"), prompt_line("fix the login page")],
        );
        write_rollout(&home, "01-00-02", "empty", &[meta("empty")]);
        fs::write(
            home.join("session_index.jsonl"),
            r#"{"id":"named","thread_name":"Login fixes","updated_at":"2026-11-21T01:00:05Z"}"#,
        )
        .unwrap();
        let title = |id| find_session(&home, id).unwrap().get_title();
        assert_eq!(title("named"), "Login fixes");
        assert_eq!(title("unnamed"), "fix the login page");
        assert_eq!(title("empty"), "Untitled session");

        // History has the name at the time it was set, after the prompt.
        let kinds: Vec<_> = find_session(&home, "named")
            .unwrap()
            .read_events()
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(matches!(kinds[0], AgentEventKind::Prompt { .. }));
        assert_eq!(
            kinds[1],
            AgentEventKind::TitleChanged {
                title: "Login fixes".into()
            }
        );
    }

    #[test]
    fn running_follows_the_newest_turn_marker() {
        let home = temp_home("running");
        let meta = |id| meta_line(id, "/p", r#""cli""#, Some("user"));
        let marker = |kind: &str| {
            format!(
                r#"{{"timestamp":"2026-11-21T01:00:02Z","type":"event_msg","payload":{{"type":"{kind}","turn_id":"t"}}}}"#
            )
        };
        let token =
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_string();
        write_rollout(
            &home,
            "01-00-00",
            "open",
            &[meta("open"), marker("task_started"), token.clone()],
        );
        write_rollout(
            &home,
            "01-00-01",
            "done",
            &[
                meta("done"),
                marker("task_started"),
                marker("task_complete"),
            ],
        );
        write_rollout(
            &home,
            "01-00-02",
            "aborted",
            &[
                meta("aborted"),
                marker("task_started"),
                marker("turn_aborted"),
            ],
        );
        write_rollout(&home, "01-00-03", "idle", &[meta("idle")]);
        // A turn start followed by a line longer than the first scan window.
        let huge = format!(
            r#"{{"type":"compacted","payload":{{"message":"{}"}}}}"#,
            "x".repeat(300 * 1024)
        );
        write_rollout(
            &home,
            "01-00-04",
            "long",
            &[meta("long"), marker("task_started"), huge],
        );
        let running = |id| find_session(&home, id).unwrap().is_running();
        assert!(running("open"));
        assert!(!running("done"));
        assert!(!running("aborted"));
        assert!(!running("idle"));
        assert!(running("long"));
    }

    #[test]
    fn attach_since_then_live() {
        use std::io::Write;
        use std::time::Duration;
        let home = temp_home("attach");
        let reply = |time: &str, text: &str| {
            format!(
                r#"{{"timestamp":"{time}","type":"event_msg","payload":{{"type":"agent_message","message":"{text}"}}}}"#
            )
        };
        let path = write_rollout(
            &home,
            "01-00-00",
            "t1",
            &[
                meta_line("t1", "/p", r#""cli""#, Some("user")),
                reply("1970-01-01T00:00:01Z", "old"),
                reply("1970-01-01T00:00:02Z", "recent"),
            ],
        );
        let session = find_session(&home, "t1").unwrap();
        let attachment = session.attach(AttachMode::Since(2000)).unwrap();
        let next = || {
            attachment
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .kind
        };
        let response = |t: &str| AgentEventKind::Response { text: t.into() };
        assert_eq!(next(), response("recent"));

        // A line written after attaching arrives, even in two writes.
        let mut file = File::options().append(true).open(&path).unwrap();
        let live = reply("1970-01-01T00:00:03Z", "live");
        let (a, b) = live.split_at(20);
        file.write_all(a.as_bytes()).unwrap();
        file.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        file.write_all(format!("{b}\n").as_bytes()).unwrap();
        assert_eq!(next(), response("live"));

        let new_only = session.attach(AttachMode::NewOnly).unwrap();
        assert!(new_only.recv_timeout(Duration::from_millis(600)).is_err());
    }

    #[test]
    fn reads_events_past_a_broken_line() {
        let home = temp_home("events");
        let path = write_rollout(
            &home,
            "01-00-00",
            "t1",
            &[
                meta_line("t1", "/p", r#""cli""#, Some("user")),
                r#"{"timestamp":"2026-11-21T01:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#.into(),
            ],
        );
        // Append a line with invalid UTF-8, then a reply.
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"\xff\"}}\n");
        bytes.extend_from_slice(
            br#"{"type":"event_msg","payload":{"type":"agent_message","message":"ok"}}"#,
        );
        bytes.push(b'\n');
        fs::write(&path, bytes).unwrap();
        let session = find_session(&home, "t1").unwrap();
        let kinds: Vec<_> = session
            .read_events()
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds.len(), 3, "{kinds:?}");
        assert_eq!(
            kinds[2],
            crate::agents::AgentEventKind::Response { text: "ok".into() }
        );
    }
}
