//! Claude Code integration.
//!
//! Reads the session transcripts Claude Code keeps under
//! `~/.claude/projects` (or `$CLAUDE_CONFIG_DIR/projects`). Nothing needs to
//! be installed into Claude Code for this; following a transcript file is
//! enough to see prompts, responses, tool calls and interrupts as they
//! happen. See [`transcript`] for the format.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::UNIX_EPOCH;

use crate::agents::{
    AgentError, AgentEvent, AgentEventKind, AgentInstance, AgentIntegration, AttachMode,
    Attachment, shorten,
};
use crate::agents::{is_running, tail};

pub mod history;
pub mod transcript;

use history::PromptHistory;
use transcript::TranscriptParser;

pub struct AgentIntegrationClaudeCode;

impl AgentIntegrationClaudeCode {
    /// Claude Code's config directory.
    pub fn config_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(PathBuf::from(home).join(".claude"))
    }

    fn history() -> PromptHistory {
        match Self::config_dir() {
            Some(dir) => PromptHistory::load(&dir.join("history.jsonl")),
            None => PromptHistory::default(),
        }
    }

    fn projects_dir() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("projects"))
    }

    /// Look up one session by its id.
    pub fn get_instance(&self, session_id: &str) -> Result<AgentInstanceClaudeCode, AgentError> {
        let projects = Self::projects_dir()
            .filter(|d| d.is_dir())
            .ok_or(AgentError::NotInstalled)?;
        let file = format!("{session_id}.jsonl");
        for project in fs::read_dir(projects)?.flatten() {
            let path = project.path().join(&file);
            if path.is_file()
                && let Some(instance) = AgentInstanceClaudeCode::load(&path)
            {
                return Ok(instance);
            }
        }
        Err(AgentError::SessionNotFound(session_id.to_string()))
    }
}

impl AgentIntegration for AgentIntegrationClaudeCode {
    fn get_name(&self) -> String {
        "Claude Code".to_string()
    }

    /// All recorded sessions, most recently active first.
    fn get_instances(&self) -> Option<Vec<impl AgentInstance>> {
        let projects = Self::projects_dir().filter(|d| d.is_dir())?;
        let mut instances: Vec<AgentInstanceClaudeCode> = fs::read_dir(projects)
            .ok()?
            .flatten()
            .filter_map(|project| fs::read_dir(project.path()).ok())
            .flat_map(|files| files.flatten())
            .map(|file| file.path())
            .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|path| AgentInstanceClaudeCode::load(&path))
            .collect();
        instances.sort_by_key(|i| std::cmp::Reverse(i.last_activity_ms));
        Some(instances)
    }

    fn install(&self) -> Result<(), String> {
        // Transcripts are read directly, so there are no hooks to install.
        if self.is_installed() {
            Ok(())
        } else {
            Err("Claude Code is not installed".to_string())
        }
    }

    fn is_installed(&self) -> bool {
        Self::projects_dir().is_some_and(|d| d.is_dir())
    }
}

/// One Claude Code session, backed by its transcript file.
#[derive(Debug, Clone)]
pub struct AgentInstanceClaudeCode {
    session_id: String,
    title: String,
    project_path: PathBuf,
    transcript_path: PathBuf,
    last_activity_ms: Option<u64>,
    running: bool,
}

impl AgentInstanceClaudeCode {
    /// Read a session's metadata from its transcript. `None` if the file is
    /// not a session transcript or the session never got a message.
    pub fn load(transcript_path: &Path) -> Option<Self> {
        let session_id = transcript_path.file_stem()?.to_str()?.to_string();
        if !is_uuid(&session_id) {
            return None;
        }
        let meta = scan_metadata(transcript_path)?;
        let last_activity_ms = fs::metadata(transcript_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        Some(Self {
            session_id,
            title: meta.title,
            project_path: meta.cwd,
            transcript_path: transcript_path.to_path_buf(),
            running: is_running(meta.turn_open, last_activity_ms),
            last_activity_ms,
        })
    }

    pub fn transcript_path(&self) -> &Path {
        &self.transcript_path
    }

    fn ensure_exists(&self) -> Result<(), AgentError> {
        if self.transcript_path.is_file() {
            Ok(())
        } else {
            Err(AgentError::SessionNotFound(self.session_id.clone()))
        }
    }
}

impl AgentInstance for AgentInstanceClaudeCode {
    fn get_title(&self) -> String {
        self.title.clone()
    }

    fn get_uuid(&self) -> String {
        // Claude Code names transcripts after the session's UUID.
        self.session_id.clone()
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
        let mut parser = TranscriptParser::new();
        let mut events = Vec::new();
        for line in BufReader::new(File::open(&self.transcript_path)?).lines() {
            events.extend(parser.parse_line(&line?));
        }
        AgentIntegrationClaudeCode::history().fill(&self.session_id, &mut events);
        Ok(events)
    }

    fn attach(&self, mode: AttachMode) -> Result<Attachment, AgentError> {
        self.ensure_exists()?;
        let path = self.transcript_path.clone();
        let session_id = self.session_id.clone();
        Ok(Attachment::spawn(move |tx, stop| {
            // History is always parsed so titles are known, but only sent
            // when asked for.
            let mut parser = TranscriptParser::new();
            let mut prompts = AgentIntegrationClaudeCode::history();
            let result = tail::follow_lines(&path, &stop, |line, is_history| {
                let mut events = parser.parse_line(line);
                if is_history && mode == AttachMode::NewOnly {
                    return true;
                }
                prompts.fill(&session_id, &mut events);
                events.into_iter().all(|e| tx.send(e).is_ok())
            });
            if let Err(e) = result {
                // The receiver only sees the channel close; there is no
                // error event, so leave a trace for debugging.
                if !stop.load(Ordering::Relaxed) {
                    eprintln!("claude code: stopped following {}: {e}", path.display());
                }
            }
        }))
    }
}

struct Metadata {
    title: String,
    cwd: PathBuf,
    /// The newest event is neither a turn end nor an interrupt.
    turn_open: bool,
}

/// Bytes read from the start of a transcript (working directory, first
/// prompt) and from its end (latest title) when listing sessions.
const SCAN_HEAD: u64 = 256 * 1024;
const SCAN_TAIL: u64 = 1024 * 1024;

/// Find the title and working directory without reading the whole
/// transcript, which can be hundreds of MB. Claude Code re-appends the
/// title lines as the session goes on, so the end of the file has the
/// current one. Only lines that can hold what we need are decoded.
fn scan_metadata(path: &Path) -> Option<Metadata> {
    let mut parser = TranscriptParser::new();
    let mut cwd: Option<PathBuf> = None;
    let mut first_prompt: Option<String> = None;

    let lines = head_and_tail_lines(path).ok()?;
    for line in &lines {
        let line = line.as_str();
        if line.contains("\"customTitle\"")
            || line.contains("\"aiTitle\"")
            || line.contains("\"type\":\"summary\"")
        {
            parser.parse_line(line);
            continue;
        }
        if cwd.is_none() && line.contains("\"cwd\"") {
            cwd = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v.get("cwd")?.as_str().map(PathBuf::from));
        }
        if first_prompt.is_none() && line.contains("\"type\":\"user\"") {
            first_prompt = parser
                .parse_line(line)
                .into_iter()
                .find_map(|e| match e.kind {
                    AgentEventKind::Prompt { text, .. } => Some(text),
                    _ => None,
                });
        }
    }

    let title = parser
        .title()
        .map(str::to_string)
        .or_else(|| first_prompt.map(|p| shorten(&p, 60)))
        .unwrap_or_else(|| "Untitled session".to_string());
    // Read backwards to the newest line that is an actual event (title and
    // bookkeeping lines come after a turn ends).
    let last_event = lines.iter().rev().find_map(|line| {
        TranscriptParser::new()
            .parse_line(line)
            .into_iter()
            .map(|e| e.kind)
            .rfind(|k| !matches!(k, AgentEventKind::TitleChanged { .. }))
    });
    let turn_open = last_event.is_some_and(|kind| {
        !matches!(
            kind,
            AgentEventKind::TurnEnd { .. } | AgentEventKind::Interrupt { .. }
        )
    });
    Some(Metadata {
        title,
        cwd: cwd?,
        turn_open,
    })
}

/// Complete lines in the first [`SCAN_HEAD`] and last [`SCAN_TAIL`] bytes of
/// `path`, or all lines if the file is small.
fn head_and_tail_lines(path: &Path) -> std::io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= SCAN_HEAD + SCAN_TAIL {
        return BufReader::new(file).lines().collect();
    }

    let mut read_lines =
        |start: u64, size: u64, skip_first: bool| -> std::io::Result<Vec<String>> {
            file.seek(SeekFrom::Start(start))?;
            let mut bytes = Vec::with_capacity(size as usize);
            (&mut file).take(size).read_to_end(&mut bytes)?;
            let text = String::from_utf8_lossy(&bytes);
            let mut lines: Vec<&str> = text.split('\n').collect();
            lines.pop(); // the part after the last newline is incomplete (or empty)
            let skip = usize::from(skip_first); // starts mid-line
            Ok(lines.into_iter().skip(skip).map(str::to_string).collect())
        };
    let mut lines = read_lines(0, SCAN_HEAD, false)?;
    lines.extend(read_lines(len - SCAN_TAIL, SCAN_TAIL, true)?);
    Ok(lines)
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}
