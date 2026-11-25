//! Signals for reviewers: patterns in a user's heartbeats that machines
//! make and people don't (decided 2026-11-23: flagged only, never
//! applied to credit automatically; reviewers decide).
//!
//! Everything here is a pure function of the stored heartbeats, computed
//! when a reviewer looks (`review.rs`), so improving a rule improves every
//! past review too. Thresholds are deliberately loose enough that ordinary
//! people rarely trip them; each flag says what it measured so a reviewer
//! can judge it.
//!
//! The ideas:
//! - People type in bursts with pauses; autotypers type at a steady rate
//!   (`metronome_typing`) or faster than anyone can (`superhuman_typing`).
//! - Mouse jigglers and their key-press variants make sparse input at
//!   near-constant intervals (`periodic_input`).
//! - A prompt's text can't appear without key presses unless something
//!   else wrote it (`typed_without_keys`, `prompt_without_input`); some
//!   agents say so themselves (`scripted_prompts`).
//! - The OS sometimes tells us input came from software (`injected_input`).
//! - Reading takes time (`instant_replies`), and people don't answer with
//!   the same delay every time (`metronome_prompts`).
//! - An agent can't change more than the folder shows (`edit_claims_exceed`).
//! - One person doesn't type on two machines at once
//!   (`overlapping_devices`), or for six hours without a break
//!   (`marathon`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

const MINUTE_MS: i64 = 60_000;
const SECONDS: usize = 60;

/// The heartbeat body as the review reads it. Lenient: older versions lack
/// the newer fields.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Body {
    pub version: u32,
    pub minute_ms: i64,
    pub os: String,
    pub agent: String,
    pub session_name: String,
    pub project: String,
    pub agent_running: bool,
    pub activity: bool,
    pub input_kinds: String,
    pub slots: Vec<Slot>,
    pub seconds: Seconds,
    pub injected: Counts,
    pub prompts: Prompts,
    pub interrupts: u32,
    pub responses: Responses,
    pub files: Vec<File>,
    /// v10: the client's version, the models and tool calls by kind.
    pub client: String,
    pub models: Vec<String>,
    pub tools: Vec<Tools>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Tools {
    pub kind: String,
    pub calls: u32,
    pub ms: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Slot {
    pub input: bool,
    pub keys: u32,
    pub mouse: u32,
    pub focus: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Seconds {
    pub keys: Vec<u16>,
    pub mouse: Vec<u16>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Counts {
    pub keys: u32,
    pub mouse: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Prompts {
    pub submitted: u32,
    pub typed_chars: u32,
    pub pasted_chars: u32,
    pub scripted: u32,
    pub at: Vec<u8>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Responses {
    pub finished: u32,
    pub chars: u32,
    pub at: Vec<ResponseAt>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ResponseAt {
    pub second: u8,
    pub chars: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct File {
    pub path: String,
    pub working_dir: Changes,
    pub agent_edits: Changes,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Changes {
    pub added: u32,
    pub removed: u32,
}

/// One stored heartbeat with what the server knows about it.
#[derive(Debug, Clone)]
pub struct Beat {
    /// Short device id (the key's first 8 hex characters).
    pub device: String,
    pub seq: i64,
    pub received_ms: i64,
    pub offline: bool,
    pub stale: bool,
    /// The server's session id.
    pub session: String,
    pub credit_ms: i64,
    pub body: Body,
}

impl Beat {
    fn minute(&self) -> i64 {
        self.body.minute_ms
    }

    fn told_apart(&self) -> bool {
        self.body.activity && matches!(self.body.input_kinds.as_str(), "exact" | "inferred")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize)]
pub struct Flag {
    pub kind: &'static str,
    /// How many times it happened in this stretch (repeats close together
    /// are merged into one flag).
    pub count: usize,
    pub severity: Severity,
    pub from_ms: i64,
    pub to_ms: i64,
    pub device: Option<String>,
    pub session: Option<String>,
    pub title: String,
    pub detail: String,
}

/// A device's input in one minute. Input is read for the whole system, so
/// every session's heartbeat of that minute carries the same counts.
#[derive(Debug, Clone)]
struct DeviceMinute {
    keys: Option<Vec<u16>>,
    mouse: Option<Vec<u16>>,
    /// Any input in any slot, from the idle time or the counts.
    any_input: bool,
    slot_keys: u32,
    slot_mouse: u32,
    injected: Counts,
    exact: bool,
    told_apart: bool,
}

impl DeviceMinute {
    fn of(beat: &Beat) -> Self {
        let full = |v: &Vec<u16>| (v.len() == SECONDS).then(|| v.clone());
        Self {
            keys: full(&beat.body.seconds.keys).filter(|_| beat.told_apart()),
            mouse: full(&beat.body.seconds.mouse).filter(|_| beat.told_apart()),
            any_input: beat
                .body
                .slots
                .iter()
                .any(|s| s.input || s.keys > 0 || s.mouse > 0),
            slot_keys: beat.body.slots.iter().map(|s| s.keys).sum(),
            slot_mouse: beat.body.slots.iter().map(|s| s.mouse).sum(),
            injected: beat.body.injected,
            exact: beat.body.input_kinds == "exact",
            told_apart: beat.told_apart(),
        }
    }
}

/// All signals for `beats` (one user's, any order).
pub fn flags(beats: &[Beat]) -> Vec<Flag> {
    let mut beats: Vec<&Beat> = beats.iter().collect();
    beats.sort_by(|a, b| (a.minute(), &a.device, a.seq).cmp(&(b.minute(), &b.device, b.seq)));

    // Per device, per minute: its input (first heartbeat of the minute).
    let mut devices: BTreeMap<&str, BTreeMap<i64, DeviceMinute>> = BTreeMap::new();
    for beat in &beats {
        if beat.body.activity {
            devices
                .entry(beat.device.as_str())
                .or_default()
                .entry(beat.minute())
                .or_insert_with(|| DeviceMinute::of(beat));
        }
    }

    let mut out = Vec::new();
    typed_without_keys(&beats, &devices, &mut out);
    prompt_without_input(&beats, &devices, &mut out);
    scripted_prompts(&beats, &mut out);
    for (device, minutes) in &devices {
        injected_input(device, minutes, &mut out);
        let runs = second_runs(minutes);
        metronome_typing(device, &runs, &mut out);
        superhuman_typing(device, &runs, &mut out);
        periodic_input(device, &runs, &mut out);
    }
    replies(&beats, &mut out);
    edit_claims(&beats, &mut out);
    overlapping_devices(&devices, &beats, &mut out);
    marathon(&beats, &mut out);
    late(&beats, &mut out);

    let mut out = merge_repeats(out);
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.from_ms.cmp(&b.from_ms)));
    out
}

/// Repeats of the same per-minute flag (kind, severity, device and session)
/// within 15 minutes become one flag with a count.
fn merge_repeats(mut flags: Vec<Flag>) -> Vec<Flag> {
    flags.sort_by(|a, b| {
        (a.kind, a.severity, &a.device, &a.session, a.from_ms)
            .cmp(&(b.kind, b.severity, &b.device, &b.session, b.from_ms))
    });
    let mut out: Vec<Flag> = Vec::new();
    for flag in flags {
        if let Some(last) = out.last_mut()
            && matches!(flag.kind, "typed_without_keys" | "prompt_without_input")
            && (last.kind, last.severity, &last.device, &last.session)
                == (flag.kind, flag.severity, &flag.device, &flag.session)
            && flag.from_ms - last.to_ms <= 15 * MINUTE_MS
        {
            last.to_ms = last.to_ms.max(flag.to_ms);
            last.count += flag.count;
            continue;
        }
        out.push(flag);
    }
    out
}

// ---------------------------------------------------------------------------
// Prompts and keys

/// How far back the key presses for a prompt may have been typed.
const TYPING_LOOKBACK_MS: i64 = 10 * MINUTE_MS;

/// Prompt text needs key presses: counted from the session's previous
/// prompt (at most 10 minutes back), so a long prompt typed slowly is
/// fine. Inferred counts miss key presses when typing fast, so they get
/// more slack.
fn typed_without_keys(
    beats: &[&Beat],
    devices: &BTreeMap<&str, BTreeMap<i64, DeviceMinute>>,
    out: &mut Vec<Flag>,
) {
    let mut previous_prompt: HashMap<(&str, &str), i64> = HashMap::new();
    for beat in beats {
        let p = &beat.body.prompts;
        if p.submitted == 0 {
            continue;
        }
        let key = (beat.device.as_str(), beat.session.as_str());
        let since = previous_prompt
            .insert(key, beat.minute())
            .map_or(beat.minute() - TYPING_LOOKBACK_MS, |t| {
                t.max(beat.minute() - TYPING_LOOKBACK_MS)
            });
        if p.typed_chars < 20 || p.submitted <= p.scripted {
            continue;
        }
        let Some(minutes) = devices.get(beat.device.as_str()) else {
            continue;
        };
        let Some(now) = minutes.get(&beat.minute()).filter(|m| m.told_apart) else {
            continue;
        };
        let keys: u32 = minutes
            .range(since..=beat.minute())
            .map(|(_, m)| m.slot_keys)
            .sum();
        let factor = if now.exact { 1.5 } else { 3.0 };
        if f64::from(keys) * factor + 10.0 < f64::from(p.typed_chars) {
            out.push(Flag {
                kind: "typed_without_keys",
                count: 1,
                severity: if now.exact {
                    Severity::High
                } else {
                    Severity::Medium
                },
                from_ms: beat.minute(),
                to_ms: beat.minute() + MINUTE_MS,
                device: Some(beat.device.clone()),
                session: Some(beat.session.clone()),
                title: "Prompt text without the key presses to type it".into(),
                detail: format!(
                    "{} typed characters, {keys} key presses since the previous prompt \
                     ({} counts). Something other than the keyboard wrote it: tmux send-keys, \
                     a pipe, a script.",
                    p.typed_chars, beat.body.input_kinds
                ),
            });
        }
    }
}

/// A prompt in a minute without any input, nor in the minute before.
fn prompt_without_input(
    beats: &[&Beat],
    devices: &BTreeMap<&str, BTreeMap<i64, DeviceMinute>>,
    out: &mut Vec<Flag>,
) {
    for beat in beats {
        let p = &beat.body.prompts;
        if p.submitted <= p.scripted || !beat.body.activity {
            continue;
        }
        let Some(minutes) = devices.get(beat.device.as_str()) else {
            continue;
        };
        let quiet = |m: i64| minutes.get(&m).is_none_or(|d| !d.any_input);
        if minutes.contains_key(&beat.minute())
            && quiet(beat.minute())
            && quiet(beat.minute() - MINUTE_MS)
        {
            out.push(Flag {
                kind: "prompt_without_input",
                count: 1,
                severity: Severity::High,
                from_ms: beat.minute(),
                to_ms: beat.minute() + MINUTE_MS,
                device: Some(beat.device.clone()),
                session: Some(beat.session.clone()),
                title: "Prompt sent while nobody touched the computer".into(),
                detail: format!(
                    "{} prompt(s) with no keyboard or mouse input in this minute or the one before.",
                    p.submitted - p.scripted
                ),
            });
        }
    }
}

/// The agent itself says a program sent these prompts.
fn scripted_prompts(beats: &[&Beat], out: &mut Vec<Flag>) {
    let mut by_session: BTreeMap<(&str, &str), (i64, i64, u32)> = BTreeMap::new();
    for beat in beats.iter().filter(|b| b.body.prompts.scripted > 0) {
        let e = by_session
            .entry((beat.session.as_str(), beat.device.as_str()))
            .or_insert((beat.minute(), beat.minute(), 0));
        e.1 = beat.minute();
        e.2 += beat.body.prompts.scripted;
    }
    for ((session, device), (from, to, n)) in by_session {
        out.push(Flag {
            kind: "scripted_prompts",
            count: 1,
            severity: Severity::Medium,
            from_ms: from,
            to_ms: to + MINUTE_MS,
            device: Some(device.to_string()),
            session: Some(session.to_string()),
            title: "Prompts sent by a program".into(),
            detail: format!(
                "{n} prompt(s) the agent marks as coming from its SDK or `claude -p`, not its \
                 interactive interface."
            ),
        });
    }
}

/// Input the OS says came from software, in stretches.
fn injected_input(device: &str, minutes: &BTreeMap<i64, DeviceMinute>, out: &mut Vec<Flag>) {
    let hits: Vec<(i64, &DeviceMinute)> = minutes
        .iter()
        .filter(|(_, m)| m.injected.keys + m.injected.mouse > 0)
        .map(|(t, m)| (*t, m))
        .collect();
    for group in stretches(&hits, |(t, _)| *t, 2 * MINUTE_MS) {
        let injected: u32 = group
            .iter()
            .map(|(_, m)| m.injected.keys + m.injected.mouse)
            .sum();
        let total: u32 = group.iter().map(|(_, m)| m.slot_keys + m.slot_mouse).sum();
        let share = f64::from(injected) / f64::from(total.max(1));
        out.push(Flag {
            kind: "injected_input",
            count: 1,
            severity: if share >= 0.5 && total >= 20 {
                Severity::High
            } else {
                Severity::Medium
            },
            from_ms: group[0].0,
            to_ms: group[group.len() - 1].0 + MINUTE_MS,
            device: Some(device.to_string()),
            session: None,
            title: "Input that came from software".into(),
            detail: format!(
                "{injected} of {total} key and mouse events ({:.0} %) were posted by a program \
                 or a virtual device (autotypers, remote control, and also key remappers).",
                share * 100.0
            ),
        });
    }
}

// ---------------------------------------------------------------------------
// Rhythm, second by second

/// A device's per-second input over consecutive minutes that all have
/// per-second counts.
struct Run {
    start_ms: i64,
    keys: Vec<u16>,
    mouse: Vec<u16>,
}

fn second_runs(minutes: &BTreeMap<i64, DeviceMinute>) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut last: Option<i64> = None;
    for (t, m) in minutes {
        let (Some(keys), Some(mouse)) = (&m.keys, &m.mouse) else {
            last = None;
            continue;
        };
        match runs.last_mut() {
            Some(run) if last == Some(t - MINUTE_MS) => {
                run.keys.extend_from_slice(keys);
                run.mouse.extend_from_slice(mouse);
            }
            _ => runs.push(Run {
                start_ms: *t,
                keys: keys.clone(),
                mouse: mouse.clone(),
            }),
        }
        last = Some(*t);
    }
    runs
}

/// Mean and coefficient of variation (standard deviation / mean).
fn mean_cv(values: impl Iterator<Item = f64> + Clone) -> (f64, f64) {
    let n = values.clone().count() as f64;
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let mean = values.clone().sum::<f64>() / n;
    let var = values.map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    (mean, if mean > 0.0 { var.sqrt() / mean } else { 0.0 })
}

/// Stretches of consecutive indices where `keep` holds: (start, length).
fn streaks(values: &[u16], keep: impl Fn(u16) -> bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, v) in values.iter().enumerate() {
        match (keep(*v), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s, i - s));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, values.len() - s));
    }
    out
}

/// People pause, speed up and slow down: per second their key counts vary
/// by 30–80 % or more. A minute or more of typing without a single quiet
/// second, at a rate that varies by less than 20 %, is a machine.
fn metronome_typing(device: &str, runs: &[Run], out: &mut Vec<Flag>) {
    // (from, to, seconds, mean, cv) of each steady stretch.
    let mut hits: Vec<(i64, i64, usize, f64, f64)> = Vec::new();
    for run in runs {
        for (start, len) in streaks(&run.keys, |k| k > 0) {
            if len < 60 {
                continue;
            }
            let window = &run.keys[start..start + len];
            let (mean, cv) = mean_cv(window.iter().map(|&k| f64::from(k)));
            if cv < 0.2 {
                let from = run.start_ms + start as i64 * 1000;
                hits.push((from, from + len as i64 * 1000, len, mean, cv));
            }
        }
    }
    // Stretches a few minutes apart are one session of autotyping.
    for group in stretches(&hits, |h| h.0, 10 * MINUTE_MS) {
        let seconds: usize = group.iter().map(|h| h.2).sum();
        let longest = group.iter().map(|h| h.2).max().unwrap_or(0);
        let mean = group.iter().map(|h| h.3 * h.2 as f64).sum::<f64>() / seconds as f64;
        let cv = group.iter().map(|h| h.4 * h.2 as f64).sum::<f64>() / seconds as f64;
        out.push(Flag {
            kind: "metronome_typing",
            count: 1,
            severity: Severity::High,
            from_ms: group[0].0,
            to_ms: group[group.len() - 1].1,
            device: Some(device.to_string()),
            session: None,
            title: "Typing at a machine-steady rhythm".into(),
            detail: format!(
                "{} min of typing in {} stretch(es) without a single pause (longest {longest} \
                 s), at {mean:.1} keys/s varying by {:.0} % from second to second (people: \
                 30–80 %).",
                seconds / 60,
                group.len(),
                cv * 100.0
            ),
        });
    }
}

/// Faster than the fastest typists (about 15 keys/s) for several seconds.
/// A held key's auto-repeat can do it too, so only medium.
fn superhuman_typing(device: &str, runs: &[Run], out: &mut Vec<Flag>) {
    for run in runs {
        for (start, len) in streaks(&run.keys, |k| k >= 20) {
            if len < 3 {
                continue;
            }
            let peak = run.keys[start..start + len]
                .iter()
                .max()
                .copied()
                .unwrap_or(0);
            let from = run.start_ms + start as i64 * 1000;
            out.push(Flag {
                kind: "superhuman_typing",
                count: 1,
                severity: Severity::Medium,
                from_ms: from,
                to_ms: from + len as i64 * 1000,
                device: Some(device.to_string()),
                session: None,
                title: "Typing faster than a person can".into(),
                detail: format!(
                    "{len} s in a row at 20 or more keys/s (peak {peak}). Autotypers do this; so \
                     does holding a key down."
                ),
            });
        }
    }
}

/// Mouse jigglers (and their key-press variants, which count as keys on
/// Windows and X11) make sparse input at near-constant intervals. Checked
/// in 10-minute windows.
fn periodic_input(device: &str, runs: &[Run], out: &mut Vec<Flag>) {
    const WINDOW: usize = 600;
    let mut hits: Vec<(i64, String)> = Vec::new();
    for run in runs {
        let active: Vec<u16> = run
            .keys
            .iter()
            .zip(&run.mouse)
            .map(|(k, m)| k.saturating_add(*m))
            .collect();
        let mut at = 0;
        while at + WINDOW <= active.len() {
            let window = &active[at..at + WINDOW];
            let events: Vec<usize> = (0..WINDOW).filter(|&i| window[i] > 0).collect();
            if events.len() >= 16 && events.len() * 4 < WINDOW {
                let gaps: Vec<f64> = events.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
                let (mean, cv) = mean_cv(gaps.iter().copied());
                if cv < 0.15 {
                    hits.push((
                        run.start_ms + at as i64 * 1000,
                        format!("every {mean:.1} s (±{:.0} %)", cv * 100.0),
                    ));
                }
            }
            at += WINDOW;
        }
    }
    for group in stretches(&hits, |(t, _)| *t, WINDOW as i64 * 1000) {
        out.push(Flag {
            kind: "periodic_input",
            count: 1,
            severity: Severity::Medium,
            from_ms: group[0].0,
            to_ms: group[group.len() - 1].0 + WINDOW as i64 * 1000,
            device: Some(device.to_string()),
            session: None,
            title: "Input at clockwork intervals".into(),
            detail: format!(
                "For {} min, sparse input arrived {}: a mouse jiggler or a timer, not a hand.",
                group.len() * 10,
                group[0].1
            ),
        });
    }
}

// ---------------------------------------------------------------------------
// Prompts after responses

/// Fastest sensible reading speed: skimming at about 700 words a minute.
const SKIM_CHARS_PER_S: f64 = 60.0;

/// The delay from each finished response to the session's next prompt,
/// with the response's length: (prompt time, delay in s, chars).
/// Per (device, session): (prompt time, delay in s, response chars).
type Delays<'a> = BTreeMap<(&'a str, &'a str), Vec<(i64, f64, u32)>>;

#[allow(clippy::type_complexity)]
fn reply_delays<'a>(beats: &[&'a Beat]) -> Delays<'a> {
    // Per (device, session): times of responses (with their chars) and prompts.
    let mut events: BTreeMap<(&'a str, &'a str), Vec<(i64, Option<u32>)>> = BTreeMap::new();
    for beat in beats {
        let list = events
            .entry((beat.device.as_str(), beat.session.as_str()))
            .or_default();
        for r in &beat.body.responses.at {
            list.push((beat.minute() + i64::from(r.second) * 1000, Some(r.chars)));
        }
        for s in &beat.body.prompts.at {
            list.push((beat.minute() + i64::from(*s) * 1000, None));
        }
    }
    let mut out = BTreeMap::new();
    for (key, mut list) in events {
        // At the same second the response came first.
        list.sort_by_key(|(t, chars)| (*t, chars.is_none()));
        let mut last: Option<(i64, u32)> = None;
        let mut delays = Vec::new();
        for (t, chars) in list {
            match chars {
                Some(c) => last = Some((t, c)),
                None => {
                    if let Some((at, c)) = last.take() {
                        delays.push((t, (t - at) as f64 / 1000.0, c));
                    }
                }
            }
        }
        out.insert(key, delays);
    }
    out
}

fn replies(beats: &[&Beat], out: &mut Vec<Flag>) {
    for ((device, session), delays) in reply_delays(beats) {
        // Answered before a long response could even be skimmed.
        let instant: Vec<&(i64, f64, u32)> = delays
            .iter()
            .filter(|(_, delay, chars)| {
                let needed = f64::from(*chars) / SKIM_CHARS_PER_S;
                needed >= 10.0 && *delay < needed / 2.0
            })
            .collect();
        if let (Some(first), Some(last)) = (instant.first(), instant.last()) {
            out.push(Flag {
                kind: "instant_replies",
                count: 1,
                severity: if instant.len() >= 3 {
                    Severity::Medium
                } else {
                    Severity::Low
                },
                from_ms: first.0,
                to_ms: last.0 + 1000,
                device: Some(device.to_string()),
                session: Some(session.to_string()),
                title: "Answers faster than the response can be read".into(),
                detail: format!(
                    "{} prompt(s) sent less than half the skimming time after a long response \
                     (e.g. {:.0} s after {} characters). Queued prompts look like this too.",
                    instant.len(),
                    first.1,
                    first.2
                ),
            });
        }
        // The same delay again and again.
        for window in delays.windows(6) {
            let (mean, cv) = mean_cv(window.iter().map(|(_, d, _)| *d));
            let spread = cv * mean;
            if mean < 120.0 && spread < 1.0 {
                out.push(Flag {
                    kind: "metronome_prompts",
                    count: 1,
                    severity: Severity::High,
                    from_ms: window[0].0,
                    to_ms: window[5].0 + 1000,
                    device: Some(device.to_string()),
                    session: Some(session.to_string()),
                    title: "Prompts on a timer".into(),
                    detail: format!(
                        "6 prompts in a row, each {mean:.1} s (±{spread:.1} s) after the response \
                         ended, whatever its length."
                    ),
                });
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Files, devices, endurance, lateness

/// The agent claims more than the working directory shows.
fn edit_claims(beats: &[&Beat], out: &mut Vec<Flag>) {
    let mut hits: Vec<(i64, &Beat, u32, u32)> = Vec::new();
    for beat in beats {
        let claimed: u32 = beat.body.files.iter().map(|f| f.agent_edits.added).sum();
        let seen: u32 = beat.body.files.iter().map(|f| f.working_dir.added).sum();
        if f64::from(claimed) > f64::from(seen) * 1.5 + 200.0 {
            hits.push((beat.minute(), beat, claimed, seen));
        }
    }
    let mut by_session: BTreeMap<&str, Vec<(i64, &Beat, u32, u32)>> = BTreeMap::new();
    for hit in hits {
        by_session
            .entry(hit.1.session.as_str())
            .or_default()
            .push(hit);
    }
    for (session, hits) in by_session {
        for group in stretches(&hits, |h| h.0, 5 * MINUTE_MS) {
            let claimed: u32 = group.iter().map(|h| h.2).sum();
            let seen: u32 = group.iter().map(|h| h.3).sum();
            out.push(Flag {
                kind: "edit_claims_exceed",
                count: 1,
                // Nothing seen at all is usually the folder watch starting
                // late (a brand-new session), not a lie.
                severity: if seen > 0 {
                    Severity::Medium
                } else {
                    Severity::Low
                },
                from_ms: group[0].0,
                to_ms: group[group.len() - 1].0 + MINUTE_MS,
                device: Some(group[0].1.device.clone()),
                session: Some(session.to_string()),
                title: "The agent claims more edits than the folder shows".into(),
                detail: format!(
                    "Edit calls claim {claimed} characters added, the project folder shows \
                     {seen}."
                ),
            });
        }
    }
}

/// Two devices with input in the same minutes.
fn overlapping_devices(
    devices: &BTreeMap<&str, BTreeMap<i64, DeviceMinute>>,
    beats: &[&Beat],
    out: &mut Vec<Flag>,
) {
    if devices.len() < 2 {
        return;
    }
    let credited: BTreeSet<i64> = beats
        .iter()
        .filter(|b| b.credit_ms > 0)
        .map(|b| b.minute())
        .collect();
    let mut active: BTreeMap<i64, Vec<&str>> = BTreeMap::new();
    for (device, minutes) in devices {
        for (t, m) in minutes {
            if m.any_input {
                active.entry(*t).or_default().push(device);
            }
        }
    }
    let both: Vec<(i64, Vec<&str>)> = active
        .into_iter()
        .filter(|(t, d)| d.len() >= 2 && credited.contains(t))
        .collect();
    for group in stretches(&both, |(t, _)| *t, 2 * MINUTE_MS) {
        let mut names: Vec<&str> = group.iter().flat_map(|(_, d)| d.iter().copied()).collect();
        names.sort();
        names.dedup();
        out.push(Flag {
            kind: "overlapping_devices",
            count: 1,
            severity: if group.len() >= 3 {
                Severity::Medium
            } else {
                Severity::Low
            },
            from_ms: group[0].0,
            to_ms: group[group.len() - 1].0 + MINUTE_MS,
            device: None,
            session: None,
            title: "Input on two computers at once".into(),
            detail: format!(
                "{} credited minute(s) with input on {} at the same time.",
                group.len(),
                names.join(" and ")
            ),
        });
    }
}

/// Six hours of credit without a ten-minute break.
fn marathon(beats: &[&Beat], out: &mut Vec<Flag>) {
    let credited: Vec<i64> = beats
        .iter()
        .filter(|b| b.credit_ms > 0)
        .map(|b| b.minute())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for group in stretches(&credited, |t| *t, 10 * MINUTE_MS) {
        let span = group[group.len() - 1] - group[0] + MINUTE_MS;
        if span >= 6 * 60 * MINUTE_MS {
            out.push(Flag {
                kind: "marathon",
                count: 1,
                severity: Severity::Medium,
                from_ms: group[0],
                to_ms: group[group.len() - 1] + MINUTE_MS,
                device: None,
                session: None,
                title: "Hours without a break".into(),
                detail: format!(
                    "{:.1} h of credited time with no break of 10 minutes.",
                    span as f64 / 3_600_000.0
                ),
            });
        }
    }
}

/// Heartbeats that arrived late: offline ones count, stale ones earned
/// nothing. Shown so reviewers see where the claimed time rests on the
/// device's clock alone.
fn late(beats: &[&Beat], out: &mut Vec<Flag>) {
    for (kind, stale) in [("stale", true), ("offline", false)] {
        let mut by_device: HashMap<&str, Vec<&Beat>> = HashMap::new();
        for beat in beats.iter().filter(|b| {
            if stale {
                b.stale
            } else {
                b.offline && !b.stale
            }
        }) {
            by_device
                .entry(beat.device.as_str())
                .or_default()
                .push(beat);
        }
        for (device, list) in by_device {
            for group in stretches(&list, |b| b.minute(), 5 * MINUTE_MS) {
                let worst = group
                    .iter()
                    .map(|b| b.received_ms - b.minute())
                    .max()
                    .unwrap_or(0);
                out.push(Flag {
                    kind,
                    count: 1,
                    severity: Severity::Low,
                    from_ms: group[0].minute(),
                    to_ms: group[group.len() - 1].minute() + MINUTE_MS,
                    device: Some(device.to_string()),
                    session: None,
                    title: if stale {
                        "Arrived too late to count".into()
                    } else {
                        "Recorded offline".into()
                    },
                    detail: format!(
                        "{} heartbeat(s) received up to {} late{}.",
                        group.len(),
                        duration(worst),
                        if stale {
                            " (more than an hour, or before the device was linked): no credit"
                        } else {
                            ""
                        }
                    ),
                });
            }
        }
    }
}

fn duration(ms: i64) -> String {
    let minutes = ms / MINUTE_MS;
    if minutes >= 120 {
        format!("{} h", minutes / 60)
    } else {
        format!("{minutes} min")
    }
}

/// Split items (sorted by `time`) into groups whose neighbours are at most
/// `gap` apart.
fn stretches<T: Clone>(items: &[T], time: impl Fn(&T) -> i64, gap: i64) -> Vec<Vec<T>> {
    let mut out: Vec<Vec<T>> = Vec::new();
    for item in items {
        match out.last_mut() {
            Some(group) if time(item) - time(&group[group.len() - 1]) <= gap => {
                group.push(item.clone())
            }
            _ => out.push(vec![item.clone()]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_795_201_380_000;

    fn beat(minute: i64, keys: Vec<u16>, mouse: Vec<u16>) -> Beat {
        let slots = (0..6)
            .map(|i| Slot {
                input: keys[i * 10..i * 10 + 10].iter().any(|&k| k > 0)
                    || mouse[i * 10..i * 10 + 10].iter().any(|&m| m > 0),
                keys: keys[i * 10..i * 10 + 10]
                    .iter()
                    .map(|&k| u32::from(k))
                    .sum(),
                mouse: mouse[i * 10..i * 10 + 10]
                    .iter()
                    .map(|&m| u32::from(m))
                    .sum(),
                focus: "agent".into(),
            })
            .collect();
        Beat {
            device: "dev1".into(),
            seq: minute,
            received_ms: T0 + minute * MINUTE_MS + 20_000,
            offline: false,
            stale: false,
            session: "s1".into(),
            credit_ms: 60_000,
            body: Body {
                version: 9,
                minute_ms: T0 + minute * MINUTE_MS,
                agent_running: true,
                activity: true,
                input_kinds: "exact".into(),
                slots,
                seconds: Seconds { keys, mouse },
                ..Body::default()
            },
        }
    }

    /// A person: bursts of 2–9 keys/s with pauses, from a fixed pseudo-random
    /// sequence.
    fn human(minute: i64) -> Beat {
        let mut x = (minute as u64).wrapping_mul(2_654_435_761) | 1;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let keys = (0..60)
            .map(|_| {
                let r = next() % 10;
                if r < 3 { 0 } else { (next() % 8 + 2) as u16 }
            })
            .collect();
        let mouse = (0..60).map(|_| (next() % 4) as u16).collect();
        beat(minute, keys, mouse)
    }

    fn kinds(flags: &[Flag]) -> Vec<&'static str> {
        flags.iter().map(|f| f.kind).collect()
    }

    #[test]
    fn a_person_raises_nothing() {
        let beats: Vec<Beat> = (0..30).map(human).collect();
        assert_eq!(kinds(&flags(&beats)), Vec::<&str>::new());
    }

    #[test]
    fn an_autotyper_is_a_metronome() {
        let beats: Vec<Beat> = (0..3)
            .map(|m| {
                beat(
                    m,
                    (0..60).map(|s| 9 + (s % 3 == 0) as u16).collect(),
                    vec![0; 60],
                )
            })
            .collect();
        let f = flags(&beats);
        assert!(kinds(&f).contains(&"metronome_typing"), "{f:?}");
        assert_eq!(f[0].severity, Severity::High);
    }

    #[test]
    fn a_jiggler_is_periodic() {
        let beats: Vec<Beat> = (0..20)
            .map(|m| {
                let mouse = (0..60).map(|s| u16::from((m * 60 + s) % 15 == 0)).collect();
                beat(m, vec![0; 60], mouse)
            })
            .collect();
        assert!(kinds(&flags(&beats)).contains(&"periodic_input"));
    }

    #[test]
    fn prompt_text_needs_keys() {
        let mut quiet = beat(0, vec![0; 60], vec![1; 60]);
        quiet.body.prompts = Prompts {
            submitted: 1,
            typed_chars: 120,
            at: vec![30],
            ..Prompts::default()
        };
        assert!(kinds(&flags(&[quiet.clone()])).contains(&"typed_without_keys"));
        // Typed for real.
        let mut typed = human(0);
        typed.body.prompts = quiet.body.prompts.clone();
        assert!(!kinds(&flags(&[typed])).contains(&"typed_without_keys"));
        // Nobody there at all.
        let mut nobody = beat(1, vec![0; 60], vec![0; 60]);
        nobody.body.prompts = quiet.body.prompts.clone();
        assert!(kinds(&flags(&[nobody])).contains(&"prompt_without_input"));
    }

    #[test]
    fn replies_on_a_timer() {
        let beats: Vec<Beat> = (0..8)
            .map(|m| {
                let mut b = human(m);
                b.body.responses.at = vec![ResponseAt {
                    second: 10,
                    chars: 3000,
                }];
                b.body.prompts.at = vec![13];
                b.body.prompts.submitted = 1;
                b
            })
            .collect();
        let k = kinds(&flags(&beats));
        assert!(k.contains(&"metronome_prompts"), "{k:?}");
        assert!(k.contains(&"instant_replies"), "{k:?}");
    }

    #[test]
    fn edits_beyond_the_folder() {
        let mut b = human(0);
        b.body.files = vec![File {
            path: "a.rs".into(),
            working_dir: Changes {
                added: 100,
                removed: 0,
            },
            agent_edits: Changes {
                added: 5000,
                removed: 0,
            },
        }];
        let f = flags(&[b]);
        assert_eq!(kinds(&f), vec!["edit_claims_exceed"]);
        assert_eq!(f[0].severity, Severity::Medium);
    }

    #[test]
    fn two_machines_at_once() {
        let a: Vec<Beat> = (0..5).map(human).collect();
        let b: Vec<Beat> = (0..5)
            .map(|m| {
                let mut b = human(m + 100);
                b.device = "dev2".into();
                b.body.minute_ms = T0 + m * MINUTE_MS;
                b
            })
            .collect();
        let all: Vec<Beat> = a.into_iter().chain(b).collect();
        assert!(kinds(&flags(&all)).contains(&"overlapping_devices"));
    }

    #[test]
    fn injected_and_late() {
        let mut b = human(0);
        b.body.injected = Counts {
            keys: 200,
            mouse: 0,
        };
        b.stale = true;
        let k = kinds(&flags(&[b]));
        assert!(k.contains(&"injected_input"));
        assert!(k.contains(&"stale"));
    }
}
