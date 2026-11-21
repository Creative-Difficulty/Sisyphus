//! Records heartbeats: one per agent session for every minute its agent
//! ran or its window was in front, built from the agent's events, the OS's
//! activity readings and the project folder's file changes. Each one is
//! signed into the device's hash chain, queued on disk and uploaded about
//! every 30 seconds while logged in.
//!
//! Every second the tracker:
//! - reads the time since the last input and the focused app (only with
//!   the user's consent),
//! - works out, per session, whether its agent's window is in front (the
//!   most recently active session wins among sessions of the same agent and
//!   project),
//! - reads new events from the sessions it follows.
//!
//! A minute is closed a few seconds after it ends (agents write their
//! files about a second late) and turned into heartbeats.
//!
//! Only one tracker runs per machine: a lock file next to the config keeps
//! a second one (another window, the `track` command) from forking the
//! chain.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sisyphus_common::heartbeat::{Focus, Heartbeat, SLOT_SECS, SLOTS};

use crate::activity::{self, AgentSession, FocusReading, Monitor, Processes};
use crate::agents::{AgentEvent, AgentEventKind, AttachMode, Attachment, Integration, ToolOutcome};
use crate::config::Config;

mod edits;
pub mod files;
mod minute;
pub mod queue;

use edits::{Claim, ClaimKind, claims_of};
use files::{ProjectWatch, diff};
use minute::{Meta, SessionMinute};
use queue::{Queue, Uploaded};

const MINUTE_MS: u64 = 60_000;
const SLOT_MS: u64 = SLOT_SECS * 1000;
/// A minute is closed this long after it ends.
const CLOSE_DELAY_MS: u64 = 5_000;
/// Config and session list are re-read this often.
const RELOAD: Duration = Duration::from_secs(10);
/// The process table is re-read this often (or when the focused window's
/// process isn't in it).
const PROCESSES_RELOAD: Duration = Duration::from_secs(5);
/// Sessions are followed while running, and this long after they were
/// last active or in front.
const FOLLOW_MS: u64 = 30 * MINUTE_MS;
/// A project folder is still watched this long after its agent stopped,
/// for writes that land late.
const WATCH_GRACE: Duration = Duration::from_secs(10);
/// An idle watch is dropped after this long.
const WATCH_DROP: Duration = Duration::from_secs(10 * 60);
const UPLOAD_EVERY: Duration = Duration::from_secs(30);

/// The running tracker. Stops when dropped.
pub struct Tracker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tracker {
    /// Start tracking on a background thread. `log` prints each heartbeat
    /// and upload to stdout. Fails if another tracker runs on this machine
    /// or the device key can't be loaded.
    pub fn start(log: bool) -> Result<Self, String> {
        let dir = Config::path()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .ok_or("no config directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("tracker.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|_| "another Sisyphus is already tracking on this machine".to_string())?;
        let key = crate::device::load_or_create().map_err(|e| e.to_string())?;
        let queue = Arc::new(Mutex::new(Queue::load(dir, key)));
        let stop = Arc::new(AtomicBool::new(false));

        {
            let queue = queue.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("upload".into())
                .spawn(move || upload_loop(&queue, &stop, log))
                .map_err(|e| e.to_string())?;
        }
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("tracker".into())
                .spawn(move || {
                    // Held for as long as the tracker runs.
                    let _lock = lock;
                    let mut state = State::new(queue, log);
                    while !stop.load(Ordering::Relaxed) {
                        let started = Instant::now();
                        state.tick(now_ms());
                        while started.elapsed() < Duration::from_secs(1)
                            && !stop.load(Ordering::Relaxed)
                        {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                })
                .map_err(|e| e.to_string())?
        };
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn minute_of(ms: u64) -> u64 {
    ms - ms % MINUTE_MS
}

fn slot_of(ms: u64) -> usize {
    ((ms % MINUTE_MS) / SLOT_MS) as usize
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Uploads the queue every 30 seconds while logged in. Never holds the
/// queue's lock during a request, so recording isn't held up.
fn upload_loop(queue: &Mutex<Queue>, stop: &AtomicBool, log: bool) {
    let mut last: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
        if last.is_some_and(|t| t.elapsed() < UPLOAD_EVERY) {
            continue;
        }
        last = Some(Instant::now());
        let config = Config::load();
        let server = config.server_url();
        if config.account().is_none() || !crate::login::is_safe_server(&server) {
            continue;
        }
        // A few rounds: big backlogs go in several batches, and a
        // re-aligned chain is sent right away.
        for _ in 0..20 {
            let Some(batch) = lock(queue).next_batch() else {
                break;
            };
            let reply = queue::send(&server, &batch);
            let outcome = lock(queue).apply(reply);
            if log {
                println!("upload: {outcome:?}");
            }
            if !matches!(outcome, Uploaded::Sent(_) | Uploaded::Realigned)
                || stop.load(Ordering::Relaxed)
            {
                break;
            }
        }
    }
}

type Key = (&'static str, String);

struct Session {
    agent: &'static str,
    id: String,
    title: String,
    project: PathBuf,
    last_activity_ms: u64,
    /// Mid-turn, from the session list and then from events.
    running: bool,
    /// Last time its agent's window was in front.
    focused_ms: Option<u64>,
    attachment: Option<Attachment>,
    /// Attaching failed; don't retry before this.
    retry_at: Option<Instant>,
    /// Edit claims of tool calls waiting for their result, by call id.
    calls: HashMap<String, Vec<Claim>>,
    minutes: BTreeMap<u64, SessionMinute>,
}

impl Session {
    fn follow(&self, now: u64) -> bool {
        self.running
            || now.saturating_sub(self.last_activity_ms) < FOLLOW_MS
            || self
                .focused_ms
                .is_some_and(|t| now.saturating_sub(t) < FOLLOW_MS)
    }

    fn minute(&mut self, minute_ms: u64) -> &mut SessionMinute {
        self.minutes.entry(minute_ms).or_default()
    }
}

struct Watch {
    watch: ProjectWatch,
    last_running: Instant,
}

struct State {
    queue: Arc<Mutex<Queue>>,
    log: bool,
    started_ms: u64,
    config: Config,
    allowed: bool,
    monitor: Monitor,
    reloaded: Option<Instant>,
    processes: Processes,
    processes_read: Instant,
    sessions: HashMap<Key, Session>,
    watches: HashMap<PathBuf, Watch>,
    /// Input per slot of each open minute.
    input: BTreeMap<u64, [bool; SLOTS]>,
    /// Which session last claimed an edit to a file, by project and path.
    claimed: HashMap<(PathBuf, String), Key>,
}

impl State {
    fn new(queue: Arc<Mutex<Queue>>, log: bool) -> Self {
        let config = Config::load();
        let allowed = config.activity_allowed == Some(true);
        Self {
            queue,
            log,
            started_ms: now_ms(),
            config,
            allowed,
            monitor: Monitor::new(allowed),
            reloaded: None,
            processes: Processes::snapshot(),
            processes_read: Instant::now(),
            sessions: HashMap::new(),
            watches: HashMap::new(),
            input: BTreeMap::new(),
            claimed: HashMap::new(),
        }
    }

    fn tick(&mut self, now: u64) {
        if self.reloaded.is_none_or(|t| t.elapsed() >= RELOAD) {
            self.reload();
            self.reloaded = Some(Instant::now());
        }
        self.follow(now);
        self.read_events(now);
        self.sample(now);
        self.watch_files(now);
        self.close_minutes(now);
    }

    /// Re-read the config and the session list.
    fn reload(&mut self) {
        self.config = Config::load();
        let allowed = self.config.activity_allowed == Some(true);
        if allowed != self.allowed {
            self.allowed = allowed;
            self.monitor = Monitor::new(allowed);
        }

        let mut listed = HashSet::new();
        for integration in &Integration::ALL {
            if !self
                .config
                .connected_agents
                .iter()
                .any(|id| id == integration.id())
            {
                continue;
            }
            let agent = integration.id();
            for instance in integration.instances().unwrap_or_default() {
                let Some(project) = instance.get_project_path() else {
                    continue;
                };
                let key = (agent, instance.get_uuid());
                listed.insert(key.clone());
                let session = self.sessions.entry(key.clone()).or_insert_with(|| Session {
                    agent,
                    id: key.1.clone(),
                    title: String::new(),
                    project: project.clone(),
                    last_activity_ms: 0,
                    running: false,
                    focused_ms: None,
                    attachment: None,
                    retry_at: None,
                    calls: HashMap::new(),
                    minutes: BTreeMap::new(),
                });
                session.title = instance.get_title();
                session.project = project;
                session.last_activity_ms = session
                    .last_activity_ms
                    .max(instance.get_last_activity_ms().unwrap_or(0));
                session.running = instance.is_running();
            }
        }
        // Forget sessions that are gone or disconnected, once their open
        // minutes are sent.
        self.sessions
            .retain(|key, s| listed.contains(key) || !s.minutes.is_empty());
        for (key, session) in &mut self.sessions {
            if !listed.contains(key) {
                session.attachment = None;
                session.running = false;
            }
        }
    }

    /// Attach to sessions worth following, detach from the rest.
    fn follow(&mut self, now: u64) {
        let since = now.saturating_sub(20_000).max(self.started_ms);
        for ((agent, _), session) in &mut self.sessions {
            let wanted = session.follow(now);
            let alive = session
                .attachment
                .as_ref()
                .is_some_and(Attachment::is_alive);
            if wanted && !alive {
                if session.retry_at.is_some_and(|t| Instant::now() < t) {
                    continue;
                }
                let Some(integration) = Integration::ALL.iter().find(|i| i.id() == *agent) else {
                    continue;
                };
                session.attachment = integration
                    .instance(&session.id)
                    .and_then(|i| i.attach(AttachMode::Since(since)))
                    .ok();
                session.retry_at = session
                    .attachment
                    .is_none()
                    .then(|| Instant::now() + RELOAD);
            } else if !wanted && session.attachment.is_some() {
                session.attachment = None;
                session.calls.clear();
            }
        }
    }

    fn read_events(&mut self, now: u64) {
        let open_from = self.open_from(now);
        let keys: Vec<Key> = self.sessions.keys().cloned().collect();
        for key in keys {
            let events: Vec<AgentEvent> = match &self.sessions[&key].attachment {
                Some(attachment) => std::iter::from_fn(|| attachment.try_recv().ok()).collect(),
                None => continue,
            };
            for event in events {
                // Late events for a closed minute go into the current one.
                let minute_ms = minute_of(event.time_ms).clamp(open_from, minute_of(now));
                self.apply(&key, minute_ms, event);
            }
        }
    }

    /// The oldest minute still open: minutes close [`CLOSE_DELAY_MS`]
    /// after they end.
    fn open_from(&self, now: u64) -> u64 {
        minute_of(now.saturating_sub(CLOSE_DELAY_MS)).max(minute_of(self.started_ms))
    }

    fn apply(&mut self, key: &Key, minute_ms: u64, event: AgentEvent) {
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        session.last_activity_ms = session.last_activity_ms.max(event.time_ms);
        match event.kind {
            AgentEventKind::Prompt { text, pasted_chars } => {
                let chars = text.chars().count();
                let pasted = pasted_chars.unwrap_or(0).min(chars);
                session.running = true;
                let minute = session.minute(minute_ms);
                minute.running = true;
                let prompts = &mut minute.prompts;
                prompts.submitted += 1;
                prompts.typed_chars = prompts.typed_chars.saturating_add(count(chars - pasted));
                prompts.pasted_chars = prompts.pasted_chars.saturating_add(count(pasted));
            }
            AgentEventKind::Response { text } => {
                let minute = session.minute(minute_ms);
                minute.running = true;
                minute.responses.finished += 1;
                minute.responses.chars = minute
                    .responses
                    .chars
                    .saturating_add(count(text.chars().count()));
            }
            AgentEventKind::ToolCall { id, name, input } => {
                session.minute(minute_ms).running = true;
                let claims = claims_of(&name, &input, &session.project);
                if !claims.is_empty() {
                    session.calls.insert(id, claims);
                }
            }
            AgentEventKind::ToolResult { id, outcome } => {
                let Some(claims) = session.calls.remove(&id) else {
                    return;
                };
                if outcome != ToolOutcome::Success {
                    return;
                }
                let project = session.project.clone();
                for claim in claims {
                    let changes = match &claim.kind {
                        ClaimKind::Replace(changes) => *changes,
                        ClaimKind::Write(lines) => {
                            let before = self
                                .watches
                                .get(&project)
                                .map(|w| w.watch.lines_before_write(&claim.path, lines))
                                .unwrap_or_default();
                            diff(&before, lines)
                        }
                    };
                    let session = self.sessions.get_mut(key).expect("checked above");
                    session
                        .minute(minute_ms)
                        .files
                        .entry(claim.path.clone())
                        .or_default()
                        .1
                        .add(changes);
                    self.claimed
                        .insert((project.clone(), claim.path), key.clone());
                }
            }
            AgentEventKind::Interrupt { .. } | AgentEventKind::TurnEnd { .. } => {
                session.running = false;
            }
            AgentEventKind::TitleChanged { title } => session.title = title,
            AgentEventKind::Command { .. }
            | AgentEventKind::Thinking { .. }
            | AgentEventKind::Compacted => {}
        }
    }

    /// One reading of input and focus, recorded into the open minute.
    fn sample(&mut self, now: u64) {
        let minute_ms = minute_of(now);
        let slot = slot_of(now);

        if let Some(idle) = self.monitor.since_last_input() {
            let at = now.saturating_sub(idle.as_millis() as u64);
            if at >= self.open_from(now) {
                self.input.entry(minute_of(at)).or_default()[slot_of(at)] = true;
            }
        }
        self.input.entry(minute_ms).or_default();

        let reading = self.monitor.focused();
        if let FocusReading::App(app) = &reading {
            let stale = self.processes_read.elapsed() >= PROCESSES_RELOAD;
            let unknown_pid = app.pid.is_some_and(|pid| self.processes.get(pid).is_none());
            if stale || unknown_pid {
                self.processes = Processes::snapshot();
                self.processes_read = Instant::now();
            }
        }
        let base = to_focus(reading.focus(None, &self.processes));

        // The session whose agent's window is in front. Where the agent
        // names the session its process runs, that's exact; otherwise the
        // process is matched by folder and, among sessions of the same agent
        // and project, the most recently active one wins.
        let mut in_front: HashMap<(&'static str, &Path), (&Key, u64)> = HashMap::new();
        if matches!(reading, FocusReading::App(_)) {
            for (key, session) in &self.sessions {
                let agent_session = AgentSession {
                    agent: session.agent,
                    project: &session.project,
                    id: &session.id,
                };
                let runs = reading.focus(Some(agent_session), &self.processes)
                    == Some(activity::Focus::Agent);
                let pair = (session.agent, session.project.as_path());
                if runs
                    && in_front
                        .get(&pair)
                        .is_none_or(|(_, best)| session.last_activity_ms > *best)
                {
                    in_front.insert(pair, (key, session.last_activity_ms));
                }
            }
        }
        let in_front: Vec<Key> = in_front.into_values().map(|(k, _)| k.clone()).collect();

        for (key, session) in &mut self.sessions {
            let front = in_front.contains(key);
            if front {
                session.focused_ms = Some(now);
            }
            if session.attachment.is_none() {
                continue;
            }
            let running = session.running;
            let minute = session.minute(minute_ms);
            minute.slots[slot].add(if front { Focus::Agent } else { base });
            if running {
                minute.running = true;
            }
        }
    }

    /// Watch the folders of running sessions and hand changes to them.
    fn watch_files(&mut self, now: u64) {
        let running: HashSet<PathBuf> = self
            .sessions
            .values()
            .filter(|s| s.running)
            .map(|s| s.project.clone())
            .collect();
        for project in &running {
            let watch = self
                .watches
                .entry(project.clone())
                .or_insert_with(|| Watch {
                    watch: ProjectWatch::start(project.clone()),
                    last_running: Instant::now(),
                });
            watch.watch.set_armed(true);
            watch.last_running = Instant::now();
        }
        self.watches
            .retain(|_, w| w.last_running.elapsed() < WATCH_DROP);

        let minute_ms = minute_of(now);
        let mut found = Vec::new();
        for (project, watch) in &self.watches {
            if watch.last_running.elapsed() >= WATCH_GRACE {
                watch.watch.set_armed(false);
            }
            for (path, changes) in watch.watch.take_changes() {
                found.push((project.clone(), path, changes));
            }
        }
        for (project, path, changes) in found {
            let Some(key) = self.owner(&project, &path) else {
                continue;
            };
            if let Some(session) = self.sessions.get_mut(&key) {
                session
                    .minute(minute_ms)
                    .files
                    .entry(path)
                    .or_default()
                    .0
                    .add(changes);
            }
        }
    }

    /// The session a file change belongs to: the one that last claimed an
    /// edit to it, otherwise the most recently active running (or, during
    /// the grace period, followed) session in that folder.
    fn owner(&self, project: &Path, path: &str) -> Option<Key> {
        if let Some(key) = self.claimed.get(&(project.to_path_buf(), path.to_string()))
            && self.sessions.contains_key(key)
        {
            return Some(key.clone());
        }
        self.sessions
            .iter()
            .filter(|(_, s)| s.project == project && s.attachment.is_some())
            .max_by_key(|(_, s)| (s.running, s.last_activity_ms))
            .map(|(k, _)| k.clone())
    }

    /// Turn minutes that ended a few seconds ago into heartbeats.
    fn close_minutes(&mut self, now: u64) {
        let open_from = self.open_from(now);
        let mut heartbeats: Vec<Heartbeat> = Vec::new();
        for session in self.sessions.values_mut() {
            let project_name = session
                .project
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            while let Some(entry) = session.minutes.first_entry() {
                if *entry.key() >= open_from {
                    break;
                }
                let (minute_ms, minute) = entry.remove_entry();
                if !minute.worth_sending() {
                    continue;
                }
                heartbeats.push(minute.into_heartbeat(Meta {
                    minute_ms,
                    agent: session.agent,
                    agent_session_id: &session.id,
                    session_name: &session.title,
                    project: &project_name,
                    activity: self.allowed,
                    input: self.input.get(&minute_ms).copied().unwrap_or_default(),
                }));
            }
        }
        self.input.retain(|minute, _| *minute >= open_from);
        self.claimed
            .retain(|(_, _), key| self.sessions.contains_key(key));

        if heartbeats.is_empty() {
            return;
        }
        heartbeats.sort_by_key(|h| h.minute_ms);
        let mut queue = lock(&self.queue);
        for heartbeat in heartbeats {
            if self.log {
                println!("{}", serde_json::to_string(&heartbeat).unwrap_or_default());
            }
            if let Err(e) = queue.push(heartbeat) {
                eprintln!("could not queue a heartbeat: {e}");
            }
        }
    }
}

fn to_focus(focus: Option<activity::Focus>) -> Focus {
    match focus {
        None => Focus::Unknown,
        Some(activity::Focus::Agent) => Focus::Agent,
        Some(activity::Focus::Terminal) => Focus::Terminal,
        Some(activity::Focus::Editor) => Focus::Editor,
        Some(activity::Focus::Browser) => Focus::Browser,
        Some(activity::Focus::Other) => Focus::Other,
        Some(activity::Focus::None) => Focus::None,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_and_slots() {
        assert_eq!(minute_of(1_795_201_385_123), 1_795_201_380_000);
        assert_eq!(slot_of(1_795_201_380_000), 0);
        assert_eq!(slot_of(1_795_201_439_999), 5);
    }
}
