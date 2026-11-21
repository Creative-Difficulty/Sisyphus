//! opencode integration.
//!
//! opencode (v2) stores everything in one SQLite database,
//! `$XDG_DATA_HOME/opencode/opencode.db` (`~/.local/share/opencode/` by
//! default). Sessions are rows of `session_v2`, their messages rows of
//! `session_message`; see [`session`] for the message format. The database
//! is opened read-only, and since it is in WAL mode, reading it while
//! opencode writes is safe. The older `session`/`message`/`part` tables are
//! from opencode v1 and were migrated into the v2 tables, so they are not
//! read.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::agents::{
    AgentError, AgentEvent, AgentInstance, AgentIntegration, AttachMode, Attachment, is_running,
    shorten,
};

pub mod history;
pub mod session;

use history::PromptHistory;
use session::SessionParser;

/// How often an attachment checks the database for changes.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct AgentIntegrationOpenCode;

impl AgentIntegrationOpenCode {
    /// Path of opencode's database.
    pub fn database_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(data.join("opencode").join("opencode.db"))
    }

    fn open() -> Result<Connection, AgentError> {
        let path = Self::database_path()
            .filter(|p| p.is_file())
            .ok_or(AgentError::NotInstalled)?;
        open_read_only(&path)
    }

    /// Look up one session by its id.
    pub fn get_instance(&self, session_id: &str) -> Result<AgentInstanceOpenCode, AgentError> {
        let conn = Self::open()?;
        let row = conn
            .query_row(
                &format!("{SESSION_QUERY} WHERE id = ?1"),
                params![session_id],
                SessionRow::from_row,
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;
        Ok(AgentInstanceOpenCode::from_session(&conn, row))
    }

    fn list(&self) -> Result<Vec<AgentInstanceOpenCode>, AgentError> {
        let conn = Self::open()?;
        // Subagent sessions have a parent; they belong to the parent's turn.
        let mut stmt = conn
            .prepare(&format!(
                "{SESSION_QUERY} WHERE parent_id IS NULL AND time_archived IS NULL ORDER BY time_updated DESC"
            ))
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], SessionRow::from_row)
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(|row| AgentInstanceOpenCode::from_session(&conn, row))
            .collect())
    }
}

impl AgentIntegration for AgentIntegrationOpenCode {
    fn get_name(&self) -> String {
        "opencode".to_string()
    }

    /// All top-level sessions, most recently active first.
    fn get_instances(&self) -> Option<Vec<impl AgentInstance>> {
        match self.list() {
            Ok(instances) => Some(instances),
            Err(AgentError::NotInstalled) => None,
            Err(e) => {
                eprintln!("opencode: could not list sessions: {e}");
                Some(Vec::new())
            }
        }
    }

    fn install(&self) -> Result<(), String> {
        // The database is read directly, so there is nothing to install.
        if self.is_installed() {
            Ok(())
        } else {
            Err("opencode is not installed".to_string())
        }
    }

    fn is_installed(&self) -> bool {
        Self::database_path().is_some_and(|p| p.is_file())
    }
}

const SESSION_QUERY: &str = "SELECT id, title, directory, time_updated FROM session_v2";

struct SessionRow {
    id: String,
    title: Option<String>,
    directory: String,
    time_updated: i64,
}

impl SessionRow {
    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            directory: row.get(2)?,
            time_updated: row.get(3)?,
        })
    }
}

/// One opencode session.
#[derive(Debug, Clone)]
pub struct AgentInstanceOpenCode {
    session_id: String,
    title: String,
    project_path: PathBuf,
    last_activity_ms: Option<u64>,
    running: bool,
    database_path: PathBuf,
}

impl AgentInstanceOpenCode {
    fn from_session(conn: &Connection, row: SessionRow) -> Self {
        let title = row
            .title
            .filter(|t| !is_placeholder_title(t))
            .or_else(|| first_prompt(conn, &row.id).map(|p| shorten(&p, 60)))
            .unwrap_or_else(|| "Untitled session".to_string());
        let last_activity_ms = u64::try_from(row.time_updated).ok();
        // Only sessions active recently can be running; skip the query for
        // the rest.
        let running = is_running(true, last_activity_ms) && turn_open(conn, &row.id);
        Self {
            session_id: row.id,
            title,
            project_path: PathBuf::from(row.directory),
            last_activity_ms,
            running,
            database_path: conn.path().map(PathBuf::from).unwrap_or_default(),
        }
    }
}

impl AgentInstance for AgentInstanceOpenCode {
    fn get_title(&self) -> String {
        self.title.clone()
    }

    fn get_uuid(&self) -> String {
        // opencode session ids (`ses_...`) are unique, though not UUIDs.
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
        let conn = open_read_only(&self.database_path)?;
        let mut poller = Poller::new(&self.session_id);
        poller.poll(&conn)
    }

    fn attach(&self, mode: AttachMode) -> Result<Attachment, AgentError> {
        let conn = open_read_only(&self.database_path)?;
        let mut poller = Poller::new(&self.session_id);
        // Read the history now so a missing session fails here, not later.
        let history = poller.poll(&conn)?;
        Ok(Attachment::spawn(move |tx, stop| {
            if !history
                .into_iter()
                .filter(|e| mode.sends_history(e))
                .all(|e| tx.send(e).is_ok())
            {
                return;
            }
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(POLL_INTERVAL);
                match poller.poll(&conn) {
                    Ok(events) => {
                        if !events.into_iter().all(|e| tx.send(e).is_ok()) {
                            return;
                        }
                    }
                    Err(AgentError::SessionNotFound(_)) => return,
                    // Usually a transient lock while opencode writes.
                    Err(_) => continue,
                }
            }
        }))
    }
}

/// Reads a session's new and changed rows on each call.
struct Poller {
    session_id: String,
    parser: SessionParser,
    prompts: PromptHistory,
    /// Highest `time_updated` seen. Rows at exactly this time are read again
    /// next poll (the parser ignores repeats), so writes landing in the same
    /// millisecond are not missed.
    watermark: i64,
}

impl Poller {
    fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            parser: SessionParser::new(),
            prompts: PromptHistory::default_path()
                .map(|p| PromptHistory::load(&p))
                .unwrap_or_default(),
            watermark: 0,
        }
    }

    fn poll(&mut self, conn: &Connection) -> Result<Vec<AgentEvent>, AgentError> {
        let mut events = Vec::new();

        let session: Option<(Option<String>, i64)> = conn
            .query_row(
                "SELECT title, time_updated FROM session_v2 WHERE id = ?1",
                params![self.session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        let Some((title, session_updated)) = session else {
            return Err(AgentError::SessionNotFound(self.session_id.clone()));
        };

        let mut stmt = conn
            .prepare_cached(
                "SELECT id, type, time_updated, data FROM session_message
                 WHERE session_id = ?1 AND time_updated >= ?2 ORDER BY seq",
            )
            .map_err(db_error)?;
        let mut rows = stmt
            .query(params![self.session_id, self.watermark])
            .map_err(db_error)?;
        let mut watermark = self.watermark;
        while let Some(row) = rows.next().map_err(db_error)? {
            let id: String = row.get(0).map_err(db_error)?;
            let kind: String = row.get(1).map_err(db_error)?;
            let updated: i64 = row.get(2).map_err(db_error)?;
            let data: String = row.get(3).map_err(db_error)?;
            watermark = watermark.max(updated);
            events.extend(
                self.parser
                    .process(&id, &kind, updated.max(0) as u64, &data),
            );
        }
        self.watermark = watermark;

        let title = title.filter(|t| !is_placeholder_title(t));
        events.extend(
            self.parser
                .set_title(title.as_deref(), session_updated.max(0) as u64),
        );
        self.prompts.fill(&mut events);
        Ok(events)
    }
}

fn open_read_only(path: &Path) -> Result<Connection, AgentError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(db_error)?;
    Ok(conn)
}

fn db_error(e: rusqlite::Error) -> AgentError {
    AgentError::Io(std::io::Error::other(e))
}

/// Titles opencode sets before it has generated a real one.
fn is_placeholder_title(title: &str) -> bool {
    title.trim().is_empty()
        || title.starts_with("New session - ")
        || title.starts_with("Child session - ")
}

/// Whether the session's newest prompt or reply step leaves the turn open:
/// a prompt waiting for a reply, or a step that hasn't finished or will be
/// followed by more tool calls. `idle` rows mark a finished turn.
fn turn_open(conn: &Connection, session_id: &str) -> bool {
    let last: Option<(String, String)> = conn
        .query_row(
            "SELECT type, data FROM session_message
             WHERE session_id = ?1 AND type IN ('user', 'assistant', 'idle')
             ORDER BY seq DESC LIMIT 1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((kind, data)) = last else {
        return false;
    };
    match kind.as_str() {
        "user" => true,
        "assistant" => {
            let Ok(data) = serde_json::from_str::<serde_json::Value>(&data) else {
                return false;
            };
            let failed = data.get("error").is_some_and(|e| !e.is_null());
            let completed = data.pointer("/time/completed").is_some();
            let more_steps = data.get("finish").and_then(|f| f.as_str()) == Some("tool-calls");
            !failed && (!completed || more_steps)
        }
        _ => false,
    }
}

fn first_prompt(conn: &Connection, session_id: &str) -> Option<String> {
    let data: String = conn
        .query_row(
            "SELECT data FROM session_message WHERE session_id = ?1 AND type = 'user' ORDER BY seq LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&data).ok()?;
    Some(value.get("text")?.as_str()?.to_string()).filter(|t| !t.trim().is_empty())
}
