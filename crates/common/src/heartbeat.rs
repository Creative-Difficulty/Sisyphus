//! A heartbeat is one minute of one agent session (heartbeat scheme v12).
//!
//! The client sends one per session for every minute its agent ran or the
//! user had the agent's window in front. The server computes attention, the
//! 0.0–1.0 multiplier and the struggle value from these; the client never
//! sends any of those.
//!
//! Nothing a heartbeat carries can be computed from another field (v12):
//! counts that follow from lists are left out, and the helpers below
//! compute them for every version. Fields only older versions have are
//! `Option`s, present exactly when the old heartbeat had them, so old
//! heartbeats still serialize (and hash) as they were signed.
//!
//! Rules, because heartbeats are hashed and signed:
//! - No floats and no maps, so serialization is deterministic.
//! - Counts, timings, coarse app classes, paths relative to the project
//!   folder (edited files and the project's own programs), the agent's
//!   session title, the client's version and the agent's model names only.
//!   Never prompt or response text, file contents, absolute paths, window
//!   titles or keystrokes.

use serde::{Deserialize, Serialize};

/// The version this crate writes.
pub const VERSION: u32 = 12;
/// The oldest version the server still accepts, so clients that haven't
/// updated yet keep uploading. v9 heartbeats lack `client`, `models` and
/// `tools`; v11 dropped `seconds`; v12 dropped what can be computed and
/// replaced `tools` and `agent_edits` with `tool_calls`.
pub const MIN_VERSION: u32 = 9;

/// Whether the server accepts heartbeats of `version`.
pub fn is_supported(version: u32) -> bool {
    (MIN_VERSION..=VERSION).contains(&version)
}

/// Seconds in one slot, and slots in one heartbeat.
pub const SLOT_SECS: u64 = 10;
pub const SLOTS: usize = 6;
/// Seconds in one heartbeat: the length of [`Seconds`]' arrays.
pub const SECONDS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// [`VERSION`] for heartbeats this crate writes; the server accepts
    /// [`MIN_VERSION`] and later.
    pub version: u32,
    /// Start of the minute (unix ms, device clock). The device clock is not
    /// trusted; the server records its own receive time separately.
    pub minute_ms: u64,
    /// The operating system the client runs on.
    pub os: Os,

    /// The agent integration's id, e.g. `claude-code`.
    pub agent: String,
    /// The agent's own session id. The server builds its session UUID from
    /// the device, `agent` and this.
    pub agent_session_id: String,
    /// The agent's session title. Visible only to the user and reviewers.
    pub session_name: String,
    /// The project folder's name only, never its path.
    pub project: String,

    /// The agent worked on a turn at some point in this minute.
    pub agent_running: bool,
    /// Whether activity was read, only in v9 to v11 heartbeats: always
    /// read since 2026-11-21. See [`Heartbeat::activity`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<bool>,
    /// How keyboard and mouse were told apart this minute (`keys` and
    /// `mouse` are 0 when `unknown`).
    pub input_kinds: InputKinds,
    /// The minute's six 10-second slots, in order.
    pub slots: [Slot; SLOTS],
    /// v9 to v11: the project's programs in front this minute. Since v12
    /// in [`Heartbeat::programs`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tested_programs: Option<Vec<String>>,
    /// v9 to v11: the project's programs running this minute. Since v12
    /// in [`Heartbeat::programs`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_programs: Option<Vec<String>>,

    pub prompts: Prompts,
    /// Times the user stopped the agent mid-turn this minute.
    pub interrupts: u32,
    pub responses: Responses,
    /// Files that changed in the working directory this minute, sorted by
    /// path. (v9 to v11 also listed files only the agent claimed.)
    pub files: Vec<FileChange>,
    /// The minute's input second by second, only in v9 and v10 heartbeats
    /// (dropped in v11, decided 2026-11-25: the slots' counts are enough).
    /// Always present in those, so they hash as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<Seconds>,
    /// Input this minute that came from software rather than a physical
    /// device (v9), as far as the system tells: events posted by apps on
    /// macOS, virtual (uinput) devices such as `ydotool` or key remappers
    /// on Linux with the input helper. 0 where it can't be told (Windows,
    /// X11). Also counted in `slots`.
    pub injected: Injected,

    // Added in v10. Absent in v9 heartbeats, and left out of the encoding
    // when empty, so v9 heartbeats hash the same as before.
    /// The Sisyphus client's version that recorded this, e.g. `0.3.0`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client: String,
    /// The models that worked in this session this minute, as the agent
    /// names them (`claude-opus-5-5`), sorted. Empty when the agent didn't
    /// produce anything this minute or doesn't say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// v10 and v11: the agent's tool calls by kind. Since v12 in
    /// `tool_calls`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolCalls>,

    // Added in v12, left out when empty.
    /// The project's own programs (executables inside the project folder,
    /// which is a git repository; not ones an agent started) that ran this
    /// minute, sorted by path. A running program alone makes the session
    /// send a heartbeat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub programs: Vec<Program>,
    /// The agent's tool calls that finished this minute (or were given up
    /// when the turn ended), in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl Heartbeat {
    /// Whether activity was read: always since v12.
    pub fn activity(&self) -> bool {
        self.activity.unwrap_or(true)
    }

    /// The project's programs that were in front (focus `testing`).
    pub fn tested_programs(&self) -> Vec<&str> {
        match &self.tested_programs {
            Some(old) => old.iter().map(String::as_str).collect(),
            None => self
                .programs
                .iter()
                .filter(|p| p.in_front)
                .map(|p| p.path.as_str())
                .collect(),
        }
    }

    /// The project's programs that ran.
    pub fn running_programs(&self) -> Vec<&str> {
        match &self.running_programs {
            Some(old) => old.iter().map(String::as_str).collect(),
            None => self.programs.iter().map(|p| p.path.as_str()).collect(),
        }
    }
}

/// One of the project's own programs that ran this minute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    /// Its executable's path relative to the project folder, with `/`
    /// separators.
    pub path: String,
    /// It was in the window in front at some point (focus `testing`).
    pub in_front: bool,
}

/// One tool call of the agent. Never the tool's input or output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub kind: ToolKind,
    /// The second within the minute it finished (0 to 59); one that
    /// finished after its minute closed counts in the next at second 0.
    pub at: u8,
    /// Milliseconds from call to result by the agent's timestamps,
    /// including the time the user took to approve or reject it.
    pub ms: u32,
    pub outcome: ToolOutcome,
    /// What it claims to have changed, per file relative to the project
    /// folder, sorted by path. Only for successful calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<EditClaim>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    Ok,
    Error,
    /// The user denied it.
    Rejected,
    /// No result before the turn ended or was interrupted.
    Unfinished,
}

/// Non-whitespace characters an edit call claims for one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditClaim {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

/// What a tool call does, coarsely. Never the tool's input or output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Runs a command.
    Shell,
    /// Changes files.
    Edit,
    /// Reads or searches files.
    Read,
    /// Searches or fetches the web.
    Web,
    /// Starts a sub-agent.
    Agent,
    /// Anything else: plans, to-do lists, MCP tools, ...
    Other,
}

/// One kind of tool call in a minute (v10 and v11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCalls {
    pub kind: ToolKind,
    /// Calls the agent started this minute.
    pub calls: u32,
    /// Milliseconds from call to result, summed over the calls that
    /// finished this minute, by the agent's own timestamps. Includes the
    /// time the user took to approve or reject a call.
    pub ms: u32,
}

/// Key presses and mouse events in each second of the minute, for the
/// whole system (never which keys). v9 and v10 only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seconds {
    /// [`SECONDS`] entries, or none without activity or when keys and mouse
    /// can't be told apart (`input_kinds` unknown).
    pub keys: Vec<u16>,
    /// Like `keys`.
    pub mouse: Vec<u16>,
}

/// Input that came from software, see [`Heartbeat::injected`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injected {
    pub keys: u32,
    pub mouse: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    /// Any keyboard or mouse input, from the OS idle time. Which key is
    /// never known.
    pub input: bool,
    /// Key presses in the slot (never which keys).
    pub keys: u32,
    /// Mouse events in the slot: movements, clicks and scrolling.
    pub mouse: u32,
    /// The app that held focus for most of the slot.
    pub focus: Focus,
}

impl Slot {
    pub const UNKNOWN: Self = Self {
        input: false,
        keys: 0,
        mouse: 0,
        focus: Focus::Unknown,
    };
}

/// The client's operating system family. Only the family, never a version,
/// distribution or machine name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Windows,
    Macos,
    Linux,
    /// Any other system the client was built for.
    Other,
}

impl Os {
    /// The system this was compiled for.
    pub const CURRENT: Self = if cfg!(target_os = "windows") {
        Self::Windows
    } else if cfg!(target_os = "macos") {
        Self::Macos
    } else if cfg!(target_os = "linux") {
        Self::Linux
    } else {
        Self::Other
    };
}

/// How a heartbeat's key and mouse counts were read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKinds {
    /// Not told apart: `keys` and `mouse` are 0, only `input` counts.
    Unknown,
    /// Guessed from the idle time and the mouse: input while the mouse
    /// neither moved nor had a button down counts as a key press (Windows,
    /// X11). Clicks and scrolling without movement may count as keys, and
    /// fast typing may count low.
    Inferred,
    /// Counted from the events themselves (macOS, Linux with the input
    /// helper).
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Focus {
    /// The window this session's agent runs in.
    Agent,
    Terminal,
    Editor,
    Browser,
    Other,
    /// A program of the session's project (an executable inside the project
    /// folder, which is a git repository): the user is testing what they
    /// build.
    Testing,
    /// Nothing has focus.
    None,
    /// Activity isn't allowed, or the system can't tell.
    Unknown,
}

/// Number of categories the prompt model scores (9 acts, then 13 tasks; the
/// order is the model's, named by `Prompts::model`).
pub const PROMPT_CLASSES: usize = 22;

/// Prompts submitted this minute. Counts and the local model's scores only,
/// never text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompts {
    /// v9 to v11 only; see [`Prompts::count`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted: Option<u32>,
    pub typed_chars: u32,
    pub pasted_chars: u32,
    /// v9 to v11 only; see [`Prompts::corrections`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrections: Option<u32>,
    /// The local prompt model's name (`sisyphus-prompt-1`), empty if no
    /// prompt was classified this minute.
    pub model: String,
    /// One entry per classified prompt, in the order they were sent: the
    /// probability of each category from 0 to 1000. Fewer entries than
    /// prompts when the model wasn't loaded.
    pub classes: Vec<[u16; PROMPT_CLASSES]>,
    /// The second within the minute (0 to 59) each prompt was sent, in
    /// order (v9): one entry per prompt. A prompt that arrived after its
    /// minute closed counts in the next one at second 0.
    pub at: Vec<u8>,
    /// Prompts the agent says came from a program, not its interactive
    /// interface (v9): Claude Code's SDK and `claude -p`. Included in the
    /// count.
    pub scripted: u32,
}

/// The correction score from which a prompt counts as a correction.
pub const CORRECTION_FROM: u16 = 500;
/// The index of `act:correction` among the prompt model's categories.
pub const CORRECTION: usize = 1;

impl Prompts {
    /// Prompts submitted this minute.
    pub fn count(&self) -> u32 {
        self.submitted.unwrap_or(self.at.len() as u32)
    }

    /// Prompts the local model scored at [`CORRECTION_FROM`] or more as
    /// corrections.
    pub fn corrections(&self) -> u32 {
        self.corrections.unwrap_or_else(|| {
            self.classes
                .iter()
                .filter(|c| c[CORRECTION] >= CORRECTION_FROM)
                .count() as u32
        })
    }
}

/// Agent responses that finished this minute. Counts only, never text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Responses {
    /// v9 to v11 only; see [`Responses::count`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<u32>,
    /// v9 to v11 only; see [`Responses::chars`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<u32>,
    /// The second within the minute each response finished, in order, with
    /// its visible characters (not thinking, not tool calls) (v9). Late
    /// ones count at second 0 like prompts.
    pub at: Vec<ResponseAt>,
}

impl Responses {
    /// Responses that finished this minute.
    pub fn count(&self) -> u32 {
        self.finished.unwrap_or(self.at.len() as u32)
    }

    /// Characters of their visible text.
    pub fn chars(&self) -> u32 {
        self.chars
            .unwrap_or_else(|| self.at.iter().map(|r| r.chars).fold(0, u32::saturating_add))
    }
}

/// One finished response: when, and how long it was.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseAt {
    pub second: u8,
    pub chars: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// Relative to the project folder, with `/` separators.
    pub path: String,
    /// Non-whitespace characters changed, as seen in the file itself.
    pub working_dir: Changes,
    /// v9 to v11: what the agent's edit calls claimed for this file. Since
    /// v12 in each tool call's `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_edits: Option<Changes>,
}

/// Non-whitespace characters added and removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    pub added: u32,
    pub removed: u32,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }

    pub fn add(&mut self, other: Changes) {
        self.added = self.added.saturating_add(other.added);
        self.removed = self.removed.saturating_add(other.removed);
    }
}
