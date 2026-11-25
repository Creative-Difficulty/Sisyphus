//! Records heartbeats: one per agent session for every minute its agent
//! ran or its window was in front, built from the agent's events, the OS's
//! activity readings and the project folder's file changes. Each one is
//! signed into the device's hash chain, queued on disk and uploaded about
//! every 30 seconds while logged in.
//!
//! Every second the tracker:
//! - reads the time since the last input, the key presses and mouse events
//!   counted (where the system tells them apart) and the focused app,
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

use sisyphus_common::attention::{self, Carry};
use sisyphus_common::heartbeat::{
    self, Changes, EditClaim, Focus, Heartbeat, Injected, PROMPT_CLASSES, ResponseAt, SLOT_SECS,
    SLOTS, ToolCall, ToolKind,
};

use crate::activity::process::{self, WindowAgent};
use crate::activity::{
    self, AgentSession, FocusReading, InputCounts, InputKinds, Monitor, Processes,
};
use crate::agents::{AgentEvent, AgentEventKind, AttachMode, Attachment, Integration, ToolOutcome};
use crate::config::Config;
use crate::prompt_model::{self, PromptModel};

mod edits;
pub mod files;
mod minute;
pub mod queue;
mod tools;

use edits::{Claim, ClaimKind, claims_of};
use files::{ProjectWatch, diff};
use minute::{Meta, MinuteInput, SessionMinute};
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
    debug: Arc<Mutex<DebugInfo>>,
}

/// What the tracker is doing right now, for the app's debug screen.
/// Updated every second. Local only.
#[derive(Debug, Clone, Default)]
pub struct DebugInfo {
    /// Which activity backends are in use.
    pub backends: String,
    /// How keys and mouse are told apart right now (`None`: they can't be).
    pub input_kinds: Option<InputKinds>,
    /// Key presses and mouse events this minute so far.
    pub minute_input: InputCounts,
    /// Time since the last keyboard or mouse input.
    pub since_input_ms: Option<u64>,
    /// The class of the app in front, as any session that doesn't run in
    /// it would see it.
    pub focus: Option<Focus>,
    /// Followed sessions, most recently active first.
    pub sessions: Vec<SessionDebug>,
    /// The latest heartbeat queued, and how many since the tracker started.
    pub last_heartbeat: Option<Heartbeat>,
    pub heartbeats: u64,
    /// Heartbeats waiting for upload.
    pub queued: usize,
    /// When the latest upload attempt happened (unix ms) and what came of it.
    pub last_upload: Option<(u64, String)>,
    /// Project folders watched for file changes.
    pub watched_folders: usize,
    /// The prompt model's name once loaded, else why it isn't.
    pub prompt_model: String,
    /// The latest prompt's scores (0–1000 per category).
    pub last_prompt_classes: Option<[u16; PROMPT_CLASSES]>,
}

/// One followed session, with attention as the server would compute it.
#[derive(Debug, Clone)]
pub struct SessionDebug {
    pub agent: &'static str,
    pub title: String,
    pub project: String,
    pub running: bool,
    /// Its agent's window is in front.
    pub in_front: bool,
    /// This heartbeat's attention, from the minute's readings so far.
    pub attention: f64,
    /// The multiplier this heartbeat would get so far (0 without the agent
    /// running).
    pub multiplier: f64,
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
        let debug = Arc::new(Mutex::new(DebugInfo::default()));

        {
            let queue = queue.clone();
            let stop = stop.clone();
            let debug = debug.clone();
            std::thread::Builder::new()
                .name("upload".into())
                .spawn(move || upload_loop(&queue, &stop, &debug, log))
                .map_err(|e| e.to_string())?;
        }
        let thread = {
            let stop = stop.clone();
            let debug = debug.clone();
            std::thread::Builder::new()
                .name("tracker".into())
                .spawn(move || {
                    // Held for as long as the tracker runs.
                    let _lock = lock;
                    let mut state = State::new(queue, debug, log);
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
            debug,
        })
    }

    /// What the tracker is doing right now.
    pub fn debug(&self) -> DebugInfo {
        lock(&self.debug).clone()
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

/// The second (0 to 59) within its minute.
fn second_of(ms: u64) -> usize {
    ((ms % MINUTE_MS) / 1000) as usize
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Uploads the queue every 30 seconds while logged in. Never holds the
/// queue's lock during a request, so recording isn't held up.
fn upload_loop(queue: &Mutex<Queue>, stop: &AtomicBool, debug: &Mutex<DebugInfo>, log: bool) {
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
            let described = match &outcome {
                Uploaded::Nothing => "nothing to send".to_string(),
                Uploaded::Sent(n) => format!("sent {n}"),
                Uploaded::Realigned => "re-aligned the chain with the server".to_string(),
                Uploaded::Early { held, .. } => format!(
                    "{held} waiting: dated after the server's time (this computer's clock is ahead)"
                ),
                Uploaded::Failed(e) => e.clone(),
            };
            lock(debug).last_upload = Some((now_ms(), described));
            // After `Early`, send what came before the refused heartbeats.
            let again = match outcome {
                Uploaded::Sent(_) | Uploaded::Realigned => true,
                Uploaded::Early { before, .. } => before > 0,
                Uploaded::Nothing | Uploaded::Failed(_) => false,
            };
            if !again || stop.load(Ordering::Relaxed) {
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
    /// The project folder (canonical) if it is a git repository, so its
    /// programs count as `testing`. Refreshed with the session list.
    git_root: Option<PathBuf>,
    /// The project's programs running now (relative paths), set only on
    /// the project's most recently active session. Refreshed every 10 s.
    programs: Vec<String>,
    /// When the latest prompt seen since tracking started was sent.
    last_prompt_ms: Option<u64>,
    attachment: Option<Attachment>,
    /// Attaching failed; don't retry before this.
    retry_at: Option<Instant>,
    /// Tool calls waiting for their result, by call id. Recorded as
    /// unfinished when the turn ends without one.
    calls: HashMap<String, OpenCall>,
    /// For tool calls without an id: a counter that makes one up.
    unnamed_calls: u64,
    /// The model the agent named last.
    model: Option<String>,
    minutes: BTreeMap<u64, SessionMinute>,
    /// Attention at the end of the latest minute sent, as the server will
    /// compute it.
    carry: Option<(u64, Carry)>,
}

impl Session {
    fn follow(&self, now: u64) -> bool {
        self.running
            || now.saturating_sub(self.last_activity_ms) < FOLLOW_MS
            || self
                .focused_ms
                .is_some_and(|t| now.saturating_sub(t) < FOLLOW_MS)
            || !self.programs.is_empty()
    }

    fn minute(&mut self, minute_ms: u64) -> &mut SessionMinute {
        self.minutes.entry(minute_ms).or_default()
    }
}

/// A tool call waiting for its result.
struct OpenCall {
    /// When it was called, by the agent's clock.
    started_ms: u64,
    kind: ToolKind,
    /// What it claims to change, if it succeeds.
    claims: Vec<Claim>,
}

struct Watch {
    watch: ProjectWatch,
    last_running: Instant,
}

struct State {
    queue: Arc<Mutex<Queue>>,
    debug: Arc<Mutex<DebugInfo>>,
    heartbeats: u64,
    last_heartbeat: Option<Heartbeat>,
    /// The latest readings, for the debug screen.
    since_input_ms: Option<u64>,
    focus: Option<Focus>,
    in_front: Vec<Key>,
    /// Each second's reading for the open minutes. Fills in the slots of a
    /// session found only after it started (opencode creates a session
    /// with its first prompt; sessions are listed every 10 s).
    front_history: std::collections::VecDeque<WindowReading>,
    log: bool,
    started_ms: u64,
    config: Config,
    monitor: Monitor,
    reloaded: Option<Instant>,
    processes: Processes,
    processes_read: Instant,
    sessions: HashMap<Key, Session>,
    watches: HashMap<PathBuf, Watch>,
    /// Input per slot of each open minute, for the whole system.
    input: BTreeMap<u64, MinuteInput>,
    /// How keys and mouse were told apart at the latest reading.
    input_kinds: Option<InputKinds>,
    /// Which session last claimed an edit to a file, by project and path.
    claimed: HashMap<(PathBuf, String), Key>,
    /// The local prompt classifier, once its file is downloaded. Loaded on
    /// the next reload after it appears.
    prompt_model: Option<PromptModel>,
    prompt_model_status: String,
    last_prompt_classes: Option<[u16; PROMPT_CLASSES]>,
}

impl State {
    fn new(queue: Arc<Mutex<Queue>>, debug: Arc<Mutex<DebugInfo>>, log: bool) -> Self {
        let config = Config::load();
        Self {
            queue,
            debug,
            heartbeats: 0,
            last_heartbeat: None,
            since_input_ms: None,
            focus: None,
            in_front: Vec::new(),
            front_history: std::collections::VecDeque::new(),
            log,
            started_ms: now_ms(),
            config,
            monitor: Monitor::new(true),
            input_kinds: None,
            reloaded: None,
            processes: Processes::snapshot(),
            processes_read: Instant::now(),
            sessions: HashMap::new(),
            watches: HashMap::new(),
            input: BTreeMap::new(),
            claimed: HashMap::new(),
            prompt_model: None,
            prompt_model_status: "not loaded".to_string(),
            last_prompt_classes: None,
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
        self.update_debug(now);
    }

    /// Re-read the config and the session list.
    fn reload(&mut self) {
        self.config = Config::load();
        self.load_prompt_model();

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
                    git_root: None,
                    programs: Vec::new(),
                    last_prompt_ms: None,
                    attachment: None,
                    retry_at: None,
                    calls: HashMap::new(),
                    unnamed_calls: 0,
                    model: None,
                    minutes: BTreeMap::new(),
                    carry: None,
                });
                session.title = instance.get_title();
                session.git_root = process::git_root(&project);
                session.project = project;
                session.last_activity_ms = session
                    .last_activity_ms
                    .max(instance.get_last_activity_ms().unwrap_or(0));
                session.running = instance.is_running();
            }
        }
        self.find_running_programs();

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

    /// Give each git project whose own program runs (not started by an
    /// agent) to its most recently active session, which then sends
    /// heartbeats for it.
    /// Load the prompt model if it's downloaded and not loaded yet.
    fn load_prompt_model(&mut self) {
        if self.prompt_model.is_some() {
            return;
        }
        if !prompt_model::is_installed() {
            self.prompt_model_status = "not downloaded".to_string();
            return;
        }
        match PromptModel::load_installed() {
            Ok(model) => {
                self.prompt_model = Some(model);
                self.prompt_model_status = prompt_model::MODEL_NAME.to_string();
            }
            Err(e) => {
                if self.log && self.prompt_model_status != e {
                    eprintln!("prompt model: {e}");
                }
                self.prompt_model_status = e;
            }
        }
    }

    fn find_running_programs(&mut self) {
        if self.processes_read.elapsed() >= PROCESSES_RELOAD {
            self.processes = Processes::snapshot();
            self.processes_read = Instant::now();
        }
        let running = self.processes.programs_running();
        let mut owner: HashMap<&Path, (&Key, u64)> = HashMap::new();
        for (key, session) in &self.sessions {
            if let Some(root) = &session.git_root
                && !process::programs_under(&running, root).is_empty()
                && owner
                    .get(root.as_path())
                    .is_none_or(|(_, best)| session.last_activity_ms > *best)
            {
                owner.insert(root, (key, session.last_activity_ms));
            }
        }
        let owners: HashSet<Key> = owner.into_values().map(|(k, _)| k.clone()).collect();
        for (key, session) in &mut self.sessions {
            session.programs = match &session.git_root {
                Some(root) if owners.contains(key) => process::programs_under(&running, root),
                _ => Vec::new(),
            };
        }
    }

    /// Attach to sessions worth following, detach from the rest.
    fn follow(&mut self, now: u64) {
        let since = now.saturating_sub(20_000).max(self.started_ms);
        let open_from = self.open_from(now);
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
                if session.attachment.is_some() {
                    backfill(session, &self.front_history, open_from);
                }
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
            AgentEventKind::Prompt {
                text,
                pasted_chars,
                scripted,
            } => {
                // Only the scores are kept, never the text.
                let classes = self
                    .prompt_model
                    .as_ref()
                    .and_then(|model| model.classify(&text))
                    .map(|probs| prompt_model::per_mille(&probs));
                if classes.is_some() {
                    self.last_prompt_classes = classes;
                }
                let chars = text.chars().count();
                let pasted = pasted_chars.unwrap_or(0).min(chars);
                session.running = true;
                session.last_prompt_ms = session.last_prompt_ms.max(Some(event.time_ms));
                let minute = session.minute(minute_ms);
                minute.running = true;
                let in_minute = minute_of(event.time_ms) == minute_ms;
                if in_minute && !scripted {
                    minute.prompt_slots[slot_of(event.time_ms)] = true;
                }
                let prompts = &mut minute.prompts;
                prompts.at.push(if in_minute {
                    second_of(event.time_ms) as u8
                } else {
                    0
                });
                if scripted {
                    prompts.scripted += 1;
                }
                prompts.typed_chars = prompts.typed_chars.saturating_add(count(chars - pasted));
                prompts.pasted_chars = prompts.pasted_chars.saturating_add(count(pasted));
                if let Some(classes) = classes {
                    prompts.model = prompt_model::MODEL_NAME.to_string();
                    prompts.classes.push(classes);
                }
            }
            AgentEventKind::Response { text } => {
                let model = session.model.clone();
                let minute = session.minute(minute_ms);
                minute.running = true;
                minute.models.extend(model);
                let chars = count(text.chars().count());
                minute.responses.at.push(ResponseAt {
                    second: if minute_of(event.time_ms) == minute_ms {
                        second_of(event.time_ms) as u8
                    } else {
                        0
                    },
                    chars,
                });
            }
            AgentEventKind::ToolCall { id, name, input } => {
                let claims = claims_of(&name, &input, &session.project);
                let kind = tools::kind_of(&name, !claims.is_empty());
                let id = if id.is_empty() {
                    session.unnamed_calls += 1;
                    format!("#{}", session.unnamed_calls)
                } else {
                    id
                };
                session.calls.insert(
                    id,
                    OpenCall {
                        started_ms: event.time_ms,
                        kind,
                        claims,
                    },
                );
                let model = session.model.clone();
                let minute = session.minute(minute_ms);
                minute.running = true;
                minute.models.extend(model);
            }
            AgentEventKind::ToolResult { id, outcome } => {
                let Some(call) = session.calls.remove(&id) else {
                    return;
                };
                let project = session.project.clone();
                let mut files: BTreeMap<String, Changes> = BTreeMap::new();
                if outcome == ToolOutcome::Success {
                    for claim in call.claims {
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
                        files.entry(claim.path.clone()).or_default().add(changes);
                        self.claimed
                            .insert((project.clone(), claim.path), key.clone());
                    }
                }
                let session = self.sessions.get_mut(key).expect("checked above");
                let outcome = match outcome {
                    ToolOutcome::Success => heartbeat::ToolOutcome::Ok,
                    ToolOutcome::Error => heartbeat::ToolOutcome::Error,
                    ToolOutcome::Rejected => heartbeat::ToolOutcome::Rejected,
                };
                record_call(
                    session,
                    minute_ms,
                    event.time_ms,
                    call.started_ms,
                    call.kind,
                    outcome,
                    files,
                );
            }
            AgentEventKind::Interrupt { .. } => {
                session.running = false;
                give_up_calls(session, minute_ms, event.time_ms);
                let minute = session.minute(minute_ms);
                minute.running = true;
                minute.interrupts += 1;
            }
            AgentEventKind::TurnEnd { .. } => {
                session.running = false;
                give_up_calls(session, minute_ms, event.time_ms);
            }
            AgentEventKind::TitleChanged { title } => session.title = title,
            AgentEventKind::Model { name } => {
                let name = model_name(&name);
                session.model = Some(name.clone());
                session.minute(minute_ms).models.insert(name);
            }
            AgentEventKind::Thinking { .. } => {
                let model = session.model.clone();
                session.minute(minute_ms).models.extend(model);
            }
            AgentEventKind::Command { .. } | AgentEventKind::Compacted => {}
        }
    }

    /// One reading of input and focus, recorded into the open minute.
    fn sample(&mut self, now: u64) {
        let minute_ms = minute_of(now);
        let slot = slot_of(now);

        let idle = self.monitor.since_last_input();
        self.since_input_ms = idle.map(|d| d.as_millis() as u64);
        if let Some(idle) = idle {
            let at = now.saturating_sub(idle.as_millis() as u64);
            if at >= self.open_from(now) {
                self.input.entry(minute_of(at)).or_default().slots[slot_of(at)].any = true;
            }
        }
        let counted = self.monitor.take_input();
        self.input_kinds = counted.map(|(_, kinds)| kinds);
        let input = self.input.entry(minute_ms).or_default();
        match counted {
            Some((counts, kinds)) => {
                let injected = Injected {
                    keys: counts.injected_keys,
                    mouse: counts.injected_mouse,
                };
                input.add_counts(second_of(now), counts.keys, counts.mouse, injected);
                input.add_kinds(kinds);
            }
            None => input.add_kinds(InputKinds::Unknown),
        }

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
        self.focus = Some(base);
        let (window_agents, programs) = match &reading {
            FocusReading::App(app) => (
                self.processes.agents_in_window(app),
                self.processes.programs_in_window(app),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        self.front_history.push_back(WindowReading {
            at: now,
            base,
            agents: window_agents,
            programs: programs.clone(),
        });
        let open_from = self.open_from(now);
        while self
            .front_history
            .front()
            .is_some_and(|r| minute_of(r.at) < open_from)
        {
            self.front_history.pop_front();
        }

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
        let mut in_front: Vec<Key> = in_front.into_values().map(|(k, _)| k.clone()).collect();

        // The agent's session manager (`claude agents`) shows background
        // sessions without saying which; the one that got the latest prompt
        // counts as in front (decided 2026-11-21).
        if let FocusReading::App(app) = &reading {
            for integration in &Integration::ALL {
                let agent = integration.id();
                if !self.processes.window_runs_manager(app, agent) {
                    continue;
                }
                let background: HashSet<&str> = self.processes.background_sessions(agent).collect();
                if let Some((key, _)) = self
                    .sessions
                    .iter()
                    .filter(|(_, s)| s.agent == agent && background.contains(s.id.as_str()))
                    .max_by_key(|(_, s)| (s.last_prompt_ms, s.last_activity_ms))
                    && !in_front.contains(key)
                {
                    in_front.push(key.clone());
                }
            }
        }

        self.in_front = in_front.clone();
        for (key, session) in &mut self.sessions {
            let front = in_front.contains(key);
            if front {
                session.focused_ms = Some(now);
            }
            if session.attachment.is_none() {
                continue;
            }
            let running = session.running;
            let tested = match &session.git_root {
                Some(root) if !front => process::programs_under(&programs, root),
                _ => Vec::new(),
            };
            let programs = session.programs.clone();
            let minute = session.minute(minute_ms);
            minute.slots[slot].add(if front {
                Focus::Agent
            } else if !tested.is_empty() {
                Focus::Testing
            } else {
                base
            });
            for path in programs {
                minute.programs.entry(path).or_default();
            }
            for path in tested {
                minute.programs.insert(path, true);
            }
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
                let heartbeat = minute.into_heartbeat(Meta {
                    minute_ms,
                    agent: session.agent,
                    agent_session_id: &session.id,
                    session_name: &session.title,
                    project: &project_name,
                    activity: true,
                    input: self.input.get(&minute_ms).copied().unwrap_or_default(),
                });
                let carry = session
                    .carry
                    .filter(|(m, _)| m + MINUTE_MS == minute_ms)
                    .map(|(_, c)| c);
                let outcome = attention::minute(&heartbeat_minute(&heartbeat), carry);
                session.carry = Some((minute_ms, outcome.carry));
                heartbeats.push(heartbeat);
            }
        }
        self.input.retain(|minute, _| *minute >= open_from);
        self.claimed
            .retain(|(_, _), key| self.sessions.contains_key(key));

        if heartbeats.is_empty() {
            return;
        }
        heartbeats.sort_by_key(|h| h.minute_ms);
        self.heartbeats += heartbeats.len() as u64;
        self.last_heartbeat = heartbeats.last().cloned();
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

impl State {
    /// Publish what the tracker is doing for the debug screen, with each
    /// followed session's attention computed by the shared formula.
    fn update_debug(&self, now: u64) {
        let current = minute_of(now);
        let mut sessions: Vec<(u64, SessionDebug)> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.attachment.is_some())
            .map(|(key, session)| {
                let mut last = session.carry;
                let mut outcome = None;
                for (&minute_ms, minute) in &session.minutes {
                    let carry = last
                        .filter(|(m, _)| m + MINUTE_MS == minute_ms)
                        .map(|(_, c)| c);
                    let upto = if minute_ms == current {
                        slot_of(now) + 1
                    } else {
                        SLOTS
                    };
                    let input = self.input.get(&minute_ms).copied().unwrap_or_default();
                    let slots = minute.slots_so_far(&input, upto, true);
                    let o = attention::minute(
                        &attention::Minute {
                            activity: true,
                            agent_running: minute.running,
                            input_kinds: input.kinds(),
                            slots: &slots,
                            prompts: minute.prompts.count(),
                            interrupts: minute.interrupts,
                        },
                        carry,
                    );
                    last = Some((minute_ms, o.carry));
                    outcome = Some(o);
                }
                let debug = SessionDebug {
                    agent: session.agent,
                    title: session.title.clone(),
                    project: session
                        .project
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    running: session.running,
                    in_front: self.in_front.contains(key),
                    attention: outcome.map_or(0.0, |o| o.attention),
                    multiplier: outcome.map_or(0.0, |o| o.multiplier),
                };
                (session.last_activity_ms, debug)
            })
            .collect();
        sessions.sort_by_key(|(t, _)| std::cmp::Reverse(*t));

        let queued = lock(&self.queue).len();
        let mut debug = lock(&self.debug);
        debug.backends = self.monitor.describe();
        debug.input_kinds = self.input_kinds;
        let now_input = self.input.get(&current).copied().unwrap_or_default();
        debug.minute_input = InputCounts {
            keys: now_input.slots.iter().map(|s| s.keys).sum(),
            mouse: now_input.slots.iter().map(|s| s.mouse).sum(),
            injected_keys: now_input.injected.keys,
            injected_mouse: now_input.injected.mouse,
        };
        debug.since_input_ms = self.since_input_ms;
        debug.focus = self.focus;
        debug.sessions = sessions.into_iter().map(|(_, s)| s).collect();
        debug.last_heartbeat = self.last_heartbeat.clone();
        debug.heartbeats = self.heartbeats;
        debug.queued = queued;
        debug.watched_folders = self.watches.len();
        debug.prompt_model = self.prompt_model_status.clone();
        debug.last_prompt_classes = self.last_prompt_classes;
    }
}

/// One second's reading of the window in front: its plain class, the agent
/// processes running in it and the executables of the other programs in it.
struct WindowReading {
    at: u64,
    base: Focus,
    agents: Vec<WindowAgent>,
    programs: Vec<PathBuf>,
}

/// Fill the slots a newly followed session has no readings for from the
/// recent window readings, so the time before it was found counts.
fn backfill(
    session: &mut Session,
    history: &std::collections::VecDeque<WindowReading>,
    open_from: u64,
) {
    let agent_session = AgentSession {
        agent: session.agent,
        project: &session.project,
        id: &session.id,
    };
    let readings: Vec<(u64, Focus, Vec<String>)> = history
        .iter()
        .filter(|r| minute_of(r.at) >= open_from)
        .map(|r| {
            if r.agents.iter().any(|a| a.runs(agent_session)) {
                return (r.at, Focus::Agent, Vec::new());
            }
            let tested = session
                .git_root
                .as_deref()
                .map(|root| process::programs_under(&r.programs, root))
                .unwrap_or_default();
            let focus = if tested.is_empty() {
                r.base
            } else {
                Focus::Testing
            };
            (r.at, focus, tested)
        })
        .collect();
    // Only slots without any reading of their own.
    let empty: HashSet<(u64, usize)> = readings
        .iter()
        .map(|(at, ..)| (minute_of(*at), slot_of(*at)))
        .filter(|(m, i)| {
            session
                .minutes
                .get(m)
                .is_none_or(|minute| minute.slots[*i].is_empty())
        })
        .collect();
    for (at, focus, tested) in readings {
        let key = (minute_of(at), slot_of(at));
        if empty.contains(&key) {
            let minute = session.minute(key.0);
            minute.slots[key.1].add(focus);
            for path in tested {
                minute.programs.insert(path, true);
            }
        }
    }
}

/// Record a finished (or given-up) tool call in the minute `minute_ms`,
/// which the event at `at_ms` was put into.
fn record_call(
    session: &mut Session,
    minute_ms: u64,
    at_ms: u64,
    started_ms: u64,
    kind: ToolKind,
    outcome: heartbeat::ToolOutcome,
    files: BTreeMap<String, Changes>,
) {
    let at = if minute_of(at_ms) == minute_ms {
        second_of(at_ms) as u8
    } else {
        0
    };
    session.minute(minute_ms).tool_calls.push(ToolCall {
        kind,
        at,
        ms: u32::try_from(at_ms.saturating_sub(started_ms)).unwrap_or(u32::MAX),
        outcome,
        files: files
            .into_iter()
            .filter(|(_, c)| !c.is_empty())
            .map(|(path, c)| EditClaim {
                path,
                added: c.added,
                removed: c.removed,
            })
            .collect(),
    });
}

/// The turn ended: tool calls still without a result never get one.
fn give_up_calls(session: &mut Session, minute_ms: u64, at_ms: u64) {
    let mut open: Vec<OpenCall> = session.calls.drain().map(|(_, call)| call).collect();
    open.sort_by_key(|c| c.started_ms);
    for call in open {
        record_call(
            session,
            minute_ms,
            at_ms,
            call.started_ms,
            call.kind,
            heartbeat::ToolOutcome::Unfinished,
            BTreeMap::new(),
        );
    }
}

/// A heartbeat as input to the attention formula.
fn heartbeat_minute(hb: &Heartbeat) -> attention::Minute<'_> {
    attention::Minute {
        activity: hb.activity(),
        agent_running: hb.agent_running,
        input_kinds: hb.input_kinds,
        slots: &hb.slots,
        prompts: hb.prompts.count(),
        interrupts: hb.interrupts,
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
        Some(activity::Focus::Testing) => Focus::Testing,
        Some(activity::Focus::None) => Focus::None,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A model's name as heartbeats carry it: trimmed, at most
/// [`MODEL_NAME_CHARS`] characters.
fn model_name(name: &str) -> String {
    name.trim().chars().take(MODEL_NAME_CHARS).collect()
}

const MODEL_NAME_CHARS: usize = 100;

#[cfg(test)]
mod tests {
    use super::*;

    fn session(project: &str) -> Session {
        Session {
            agent: "opencode",
            id: "ses_new".into(),
            title: String::new(),
            project: PathBuf::from(project),
            last_activity_ms: 0,
            running: true,
            focused_ms: None,
            git_root: None,
            programs: Vec::new(),
            last_prompt_ms: None,
            attachment: None,
            retry_at: None,
            calls: HashMap::new(),
            unnamed_calls: 0,
            model: None,
            minutes: BTreeMap::new(),
            carry: None,
        }
    }

    #[test]
    fn tool_calls_are_recorded_when_they_finish() {
        let m = 1_795_201_380_000;
        let mut s = session("/nonexistent/test");
        record_call(
            &mut s,
            m,
            m + 12_500,
            m + 11_000,
            ToolKind::Edit,
            heartbeat::ToolOutcome::Ok,
            BTreeMap::from([
                (
                    "b.rs".to_string(),
                    Changes {
                        added: 3,
                        removed: 1,
                    },
                ),
                ("a.rs".to_string(), Changes::default()),
            ]),
        );
        // Started in the minute before; given up when the turn ended.
        s.calls.insert(
            "c".into(),
            OpenCall {
                started_ms: m - 5_000,
                kind: ToolKind::Shell,
                claims: Vec::new(),
            },
        );
        give_up_calls(&mut s, m, m + 30_000);
        assert!(s.calls.is_empty());
        let calls = &s.minutes[&m].tool_calls;
        assert_eq!(
            calls[0],
            ToolCall {
                kind: ToolKind::Edit,
                at: 12,
                ms: 1_500,
                outcome: heartbeat::ToolOutcome::Ok,
                files: vec![EditClaim {
                    path: "b.rs".into(),
                    added: 3,
                    removed: 1
                }],
            }
        );
        assert_eq!(calls[1].outcome, heartbeat::ToolOutcome::Unfinished);
        assert_eq!((calls[1].at, calls[1].ms), (30, 35_000));
        // A result arriving after its minute closed counts at second 0.
        record_call(
            &mut s,
            m + 60_000,
            m + 10_000,
            m,
            ToolKind::Read,
            heartbeat::ToolOutcome::Error,
            BTreeMap::new(),
        );
        assert_eq!(s.minutes[&(m + 60_000)].tool_calls[0].at, 0);
    }

    #[test]
    fn a_new_session_gets_the_readings_from_before_it_was_found() {
        let m = 1_795_201_380_000;
        let window = vec![WindowAgent {
            agent: "opencode",
            cwd: Some(PathBuf::from("/nonexistent/test")),
            session: None,
        }];
        let mut history = std::collections::VecDeque::new();
        for s in 0..25 {
            // Seconds 0-14: the opencode window; 15-24: the browser.
            let agents = if s < 15 { window.clone() } else { Vec::new() };
            history.push_back(WindowReading {
                at: m + s * 1000,
                base: Focus::Browser,
                agents,
                programs: Vec::new(),
            });
        }
        let mut s = session("/nonexistent/test");
        // Slot 2 already has its own reading.
        s.minute(m).slots[2].add(Focus::Terminal);
        backfill(&mut s, &history, m);
        let minute = &s.minutes[&m];
        assert_eq!(minute.focus(0), Focus::Agent);
        // Slot 1: 5 readings in the window, 5 in the browser; the latest wins.
        assert_eq!(minute.focus(1), Focus::Browser);
        assert_eq!(minute.focus(2), Focus::Terminal);
        assert_eq!(minute.focus(3), Focus::Unknown);

        // Another project's session doesn't get the window.
        let mut other = session("/nonexistent/other");
        backfill(&mut other, &history, m);
        assert_eq!(other.minutes[&m].focus(0), Focus::Browser);
    }

    #[test]
    fn programs_of_a_git_project_count_as_testing() {
        let m = 1_795_201_380_000;
        let mut history = std::collections::VecDeque::new();
        for s in 0..10 {
            history.push_back(WindowReading {
                at: m + s * 1000,
                base: Focus::Other,
                agents: Vec::new(),
                programs: vec![PathBuf::from("/nonexistent/test/target/debug/game")],
            });
        }
        let mut s = session("/nonexistent/test");
        s.git_root = Some(PathBuf::from("/nonexistent/test"));
        backfill(&mut s, &history, m);
        let minute = &s.minutes[&m];
        assert_eq!(minute.focus(0), Focus::Testing);
        assert_eq!(
            minute.programs.iter().collect::<Vec<_>>(),
            [(&"target/debug/game".to_string(), &true)]
        );

        // Not a git repository: just another app.
        let mut plain = session("/nonexistent/test");
        backfill(&mut plain, &history, m);
        assert_eq!(plain.minutes[&m].focus(0), Focus::Other);
        assert!(plain.minutes[&m].programs.is_empty());
    }

    #[test]
    fn minutes_and_slots() {
        assert_eq!(minute_of(1_795_201_385_123), 1_795_201_380_000);
        assert_eq!(slot_of(1_795_201_380_000), 0);
        assert_eq!(slot_of(1_795_201_439_999), 5);
    }
}
