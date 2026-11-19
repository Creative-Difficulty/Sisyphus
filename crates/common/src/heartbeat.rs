//! A heartbeat means "the user was active at this moment".
//!
//! The server adds up the gaps between consecutive heartbeats, ignoring gaps
//! longer than a timeout, to get time spent (the WakaTime/Hackatime model).
//! Each heartbeat also says whether a human or an AI agent was doing the work,
//! and counts how much was typed by the user vs pasted vs inserted by AI.
//!
//! Rules, because heartbeats are hashed and signed:
//! - No floats and no maps, so serialization is deterministic.
//! - Names and counts only. Never file contents, file paths, window titles
//!   or keystrokes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// When the activity happened (unix ms, device clock). The device clock is
    /// not trusted; the server records its own receive time separately.
    pub time_ms: u64,
    /// Which collector sent this heartbeat.
    pub source: Source,
    /// Whether a human or an AI agent was doing the work.
    pub category: Category,
    /// Application the activity happened in, e.g. "vscode", "nvim",
    /// "claude-code", "firefox".
    pub app: String,
    pub project: Option<String>,
    pub language: Option<String>,
    /// True if a file was saved.
    pub is_write: bool,
    /// What changed since this collector's previous heartbeat.
    pub edits: Edits,
}

impl Heartbeat {
    /// A heartbeat with no edits. Set the other fields directly.
    pub fn new(time_ms: u64, source: Source, category: Category, app: impl Into<String>) -> Self {
        Self {
            time_ms,
            source,
            category,
            app: app.into(),
            project: None,
            language: None,
            is_write: false,
            edits: Edits::default(),
        }
    }
}

/// Which collector sent a heartbeat. The server can require two independent
/// sources to agree, e.g. the editor plugin says the user is typing and the
/// OS daemon says the editor is focused, and discount time only one vouches for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// An editor/IDE plugin.
    Editor,
    /// A hook inside an AI tool (Claude Code, Cursor, ...).
    AiTool,
    /// The client daemon reading focus and input presence from the OS. It only
    /// sends heartbeats while an editor or terminal is focused, to corroborate
    /// the other sources.
    OsDaemon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// A human working on code.
    Coding,
    /// A human writing a prompt to an AI tool. `edits.typed_chars` is its length.
    AiPrompt,
    /// An AI agent working on its own, sent periodically while it runs.
    /// Counts as AI time, not human effort.
    AiAgent,
}

/// Edit counts since the collector's previous heartbeat. Separating typed,
/// pasted and AI-inserted text is what distinguishes human work from
/// accepted output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edits {
    pub typed_chars: u32,
    pub pasted_chars: u32,
    pub ai_chars: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
}
