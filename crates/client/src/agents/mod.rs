use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::agents::claude_code::AgentIntegrationClaudeCode;
use crate::agents::codex::AgentIntegrationCodex;
use crate::agents::opencode::AgentIntegrationOpenCode;

pub mod claude_code;
pub mod codex;
pub mod opencode;
mod tail;
pub(crate) mod time;

pub trait AgentInstance {
    fn get_title(&self) -> String;
    fn get_uuid(&self) -> String; // UUID must be unique for the sisyphus client instance

    /// Directory the agent was working in, if known.
    fn get_project_path(&self) -> Option<PathBuf>;
    /// Last time the session was written to (unix ms), if known.
    fn get_last_activity_ms(&self) -> Option<u64>;
    /// True while the agent is working on a turn rather than waiting for
    /// the user, as of when the session was loaded.
    fn is_running(&self) -> bool;

    /// Read every event the session has recorded so far.
    fn read_events(&self) -> Result<Vec<AgentEvent>, AgentError>;
    /// Start listening to the session. Events arrive on the returned
    /// [`Attachment`] until it is dropped.
    fn attach(&self, mode: AttachMode) -> Result<Attachment, AgentError>;
}

// This has been written by a human
pub trait AgentIntegration: Sync {
    fn get_name(&self) -> String;
    fn get_instances(&self) -> Option<Vec<impl AgentInstance>>; // None if not installed
    fn install(&self) -> Result<(), String>; // Maybe replace with an error type
    fn is_installed(&self) -> bool;
}

pub enum Integration {
    ClaudeCode(AgentIntegrationClaudeCode),
    OpenCode(AgentIntegrationOpenCode),
    Codex(AgentIntegrationCodex),
}

impl Integration {
    pub const ALL: [Integration; 3] = [
        Integration::ClaudeCode(AgentIntegrationClaudeCode),
        Integration::OpenCode(AgentIntegrationOpenCode),
        Integration::Codex(AgentIntegrationCodex),
    ];

    pub fn name(&self) -> String {
        match self {
            Integration::ClaudeCode(i) => i.get_name(),
            Integration::OpenCode(i) => i.get_name(),
            Integration::Codex(i) => i.get_name(),
        }
    }

    /// Stable id, used in the config file. Never change an existing one.
    pub fn id(&self) -> &'static str {
        match self {
            Integration::ClaudeCode(_) => "claude-code",
            Integration::OpenCode(_) => "opencode",
            Integration::Codex(_) => "codex",
        }
    }

    pub fn is_installed(&self) -> bool {
        match self {
            Integration::ClaudeCode(i) => i.is_installed(),
            Integration::OpenCode(i) => i.is_installed(),
            Integration::Codex(i) => i.is_installed(),
        }
    }

    pub fn install(&self) -> Result<(), String> {
        match self {
            Integration::ClaudeCode(i) => i.install(),
            Integration::OpenCode(i) => i.install(),
            Integration::Codex(i) => i.install(),
        }
    }

    /// The integration's sessions as trait objects. `None` if not installed.
    pub fn instances(&self) -> Option<Vec<Box<dyn AgentInstance + '_>>> {
        fn boxed<'a>(v: Vec<impl AgentInstance + 'a>) -> Vec<Box<dyn AgentInstance + 'a>> {
            v.into_iter()
                .map(|i| Box::new(i) as Box<dyn AgentInstance>)
                .collect()
        }
        match self {
            Integration::ClaudeCode(i) => i.get_instances().map(boxed),
            Integration::OpenCode(i) => i.get_instances().map(boxed),
            Integration::Codex(i) => i.get_instances().map(boxed),
        }
    }

    /// Look up one session by id.
    pub fn instance(&self, id: &str) -> Result<Box<dyn AgentInstance>, AgentError> {
        Ok(match self {
            Integration::ClaudeCode(i) => Box::new(i.get_instance(id)?),
            Integration::OpenCode(i) => Box::new(i.get_instance(id)?),
            Integration::Codex(i) => Box::new(i.get_instance(id)?),
        })
    }
}

/// A session counts as running only if it was active this recently, so an
/// agent that crashed mid-turn doesn't show as running forever.
const RUNNING_TIMEOUT_MS: u64 = 15 * 60 * 1000;

/// Whether a session is still working on a turn: the turn is open and the
/// session was written to recently.
pub(crate) fn is_running(turn_open: bool, last_activity_ms: Option<u64>) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    turn_open && last_activity_ms.is_some_and(|t| now.saturating_sub(t) < RUNNING_TIMEOUT_MS)
}

/// Size and modification time of a file, to notice when it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileStamp {
    pub(crate) fn of(path: &std::path::Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// First line of `text`, cut to `max` characters.
pub(crate) fn shorten(text: &str, max: usize) -> String {
    let line = text.trim().lines().next().unwrap_or("");
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let cut: String = line.chars().take(max - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

/// Where an [`Attachment`] starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachMode {
    /// Send every event already in the session first, then new ones.
    ReplayHistory,
    /// Only send events that happen after attaching.
    NewOnly,
    /// Send the session's events from this time on (unix ms, as the agent
    /// recorded it), then new ones. For attaching a moment after something
    /// may already have happened.
    Since(u64),
}

impl AttachMode {
    /// Whether an event already in the session when attaching is sent.
    pub fn sends_history(self, event: &AgentEvent) -> bool {
        match self {
            AttachMode::ReplayHistory => true,
            AttachMode::NewOnly => false,
            AttachMode::Since(ms) => event.time_ms >= ms,
        }
    }
}

/// Something that happened in an agent session.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEvent {
    /// When it happened (unix ms), as recorded by the agent.
    pub time_ms: u64,
    pub kind: AgentEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEventKind {
    /// The user sent a prompt.
    Prompt {
        text: String,
        /// Characters of `text` that were pasted, as recorded in the agent's
        /// prompt history. Agents only record large pastes (shown as a
        /// placeholder like `[Pasted text #1 +40 lines]`), so small pastes
        /// count as typed. `None` if the prompt is not in the history.
        pasted_chars: Option<usize>,
    },
    /// The user ran a slash command such as `/clear`.
    Command { name: String, args: String },
    /// The agent wrote text to the user.
    Response { text: String },
    /// The agent's reasoning. `text` may be empty when the agent hides it.
    Thinking { text: String },
    /// The agent called a tool.
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A tool call finished, failed, or was rejected by the user.
    ToolResult { id: String, outcome: ToolOutcome },
    /// The user stopped the agent mid-turn.
    Interrupt { during_tool_use: bool },
    /// The agent finished its turn and is waiting for the user.
    TurnEnd { duration_ms: Option<u64> },
    /// The conversation was summarized to free up context.
    Compacted,
    /// The session's title changed.
    TitleChanged { title: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutcome {
    Success,
    Error,
    /// The user denied permission to run the tool.
    Rejected,
}

#[derive(Debug)]
pub enum AgentError {
    NotInstalled,
    /// The session no longer exists.
    SessionNotFound(String),
    Io(io::Error),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::NotInstalled => write!(f, "agent is not installed"),
            AgentError::SessionNotFound(id) => write!(f, "session {id} not found"),
            AgentError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<io::Error> for AgentError {
    fn from(e: io::Error) -> Self {
        AgentError::Io(e)
    }
}

/// A live connection to an agent session. A background worker pushes
/// [`AgentEvent`]s into it; dropping it stops the worker.
pub struct Attachment {
    events: Receiver<AgentEvent>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Attachment {
    /// Run `worker` on a new thread. It should send events until the stop
    /// flag is set or sending fails, checking the flag at least every few
    /// hundred milliseconds so dropping the attachment doesn't hang.
    pub fn spawn<F>(worker: F) -> Self
    where
        F: FnOnce(Sender<AgentEvent>, Arc<AtomicBool>) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let worker = std::thread::spawn(move || worker(tx, flag));
        Self {
            events: rx,
            stop,
            worker: Some(worker),
        }
    }

    /// Wait for the next event. `None` once the worker has exited.
    pub fn recv(&self) -> Option<AgentEvent> {
        self.events.recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<AgentEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<AgentEvent, TryRecvError> {
        self.events.try_recv()
    }

    /// Blocking iterator over events.
    pub fn iter(&self) -> impl Iterator<Item = AgentEvent> + '_ {
        self.events.iter()
    }

    /// True while the worker is still running.
    pub fn is_alive(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
