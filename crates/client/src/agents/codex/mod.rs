//! Codex integration.
//!
//! Reads the session files the Codex CLI keeps under
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread id>.jsonl`
//! (`CODEX_HOME` defaults to `~/.codex`). Nothing is installed into Codex.
//! Each file's first line is `session_meta`, naming the thread, its working
//! directory and where it came from.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::agents::{
    AgentError, AgentEvent, AgentInstance, AgentIntegration, AttachMode, Attachment,
};

pub mod rollout;

use rollout::RolloutParser;

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
    project_path: PathBuf,
    rollout_path: PathBuf,
    last_activity_ms: Option<u64>,
    running: bool,
}

impl AgentInstanceCodex {
    /// Read a session from its rollout file. `None` if it isn't a rollout,
    /// or isn't a thread the user drove (see [`Meta::is_user_thread`]).
    pub fn load(rollout_path: &Path) -> Option<Self> {
        let meta = Meta::read(rollout_path)?;
        if !meta.is_user_thread() {
            return None;
        }
        Some(Self {
            thread_id: meta.id,
            title: "Untitled session".to_string(),
            project_path: meta.cwd,
            rollout_path: rollout_path.to_path_buf(),
            last_activity_ms: modified_ms(rollout_path),
            running: false,
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
        Ok(events)
    }

    fn attach(&self, _mode: AttachMode) -> Result<Attachment, AgentError> {
        self.ensure_exists()?;
        Ok(Attachment::spawn(|_, _| {}))
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
    let mut instances: Vec<AgentInstanceCodex> = rollout_files(&home.join("sessions"))
        .iter()
        .filter_map(|path| AgentInstanceCodex::load(path))
        .collect();
    instances.sort_by_key(|i| std::cmp::Reverse(i.last_activity_ms));
    let mut seen = std::collections::HashSet::new();
    instances.retain(|i| seen.insert(i.thread_id.clone()));
    instances
}

fn find_session(home: &Path, thread_id: &str) -> Option<AgentInstanceCodex> {
    rollout_files(&home.join("sessions"))
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(thread_id))
        })
        .filter_map(|p| AgentInstanceCodex::load(p))
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
