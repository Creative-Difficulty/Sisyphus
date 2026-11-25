//! The fraud squad's view (decided 2026-11-23): any user's heartbeats over
//! a time range with what they were credited and the signals from
//! `signals.rs`, for the reviewer page `/review` ("Pulse").
//!
//! Reviewers are the users whose Slack ID is listed in the `REVIEWERS`
//! environment variable (comma-separated). Everyone else gets 403.
//!
//! - `GET /api/review/users?q=…`: find users by name, Slack ID or id.
//! - `GET /api/review/timeline?user=…&from=…&to=…`: their heartbeats
//!   (claimed minute in `[from, to)`, at most 7 days), flags and a summary.

use std::collections::{BTreeMap, HashSet};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::signals::{self, Beat, Flag, Severity};
use crate::{AppState, auth, credit, now_ms};

const MAX_RANGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The Slack IDs of reviewers, from `REVIEWERS`.
pub fn reviewers_from_env() -> HashSet<String> {
    std::env::var("REVIEWERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether the user `id` is a reviewer.
pub async fn is_reviewer(state: &AppState, user_id: i64) -> bool {
    if state.reviewers.is_empty() {
        return false;
    }
    let slack: Option<String> = sqlx::query("SELECT slack_id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|row| row.get("slack_id"));
    slack.is_some_and(|s| state.reviewers.contains(&s))
}

async fn reviewer(state: &AppState, headers: &HeaderMap) -> Result<auth::User, Box<Response>> {
    let Some(user) = auth::current_user(state, headers).await else {
        return Err(Box::new(error(StatusCode::UNAUTHORIZED, "not logged in")));
    };
    if !is_reviewer(state, user.id).await {
        return Err(Box::new(error(StatusCode::FORBIDDEN, "not a reviewer")));
    }
    Ok(user)
}

#[derive(Serialize)]
struct ApiError {
    error: &'static str,
}

fn error(status: StatusCode, error: &'static str) -> Response {
    (status, Json(ApiError { error })).into_response()
}

fn failed(e: sqlx::Error) -> Response {
    eprintln!("review failed: {e}");
    error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

// ---------------------------------------------------------------------------
// Users

#[derive(Deserialize)]
pub struct UserQuery {
    #[serde(default)]
    q: String,
}

#[derive(Serialize)]
struct UserRow {
    id: i64,
    name: Option<String>,
    slack_id: Option<String>,
    devices: i64,
    last_seen_ms: Option<i64>,
    credited_7d_ms: i64,
    heartbeats_7d: i64,
}

pub async fn users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Response {
    if let Err(r) = reviewer(&state, &headers).await {
        return *r;
    }
    let q = query.q.trim();
    let since = now_ms() - WEEK_MS;
    let rows = sqlx::query(
        "SELECT u.id, u.name, u.slack_id,
                (SELECT count(*) FROM devices d WHERE d.user_id = u.id) AS devices,
                (SELECT max(d.last_seen_ms) FROM devices d WHERE d.user_id = u.id) AS last_seen,
                (SELECT coalesce(sum(m.credit_ms), 0)::bigint FROM session_minutes m
                  WHERE m.user_id = u.id AND m.minute_ms >= $2) AS credited,
                (SELECT count(*) FROM heartbeats h
                  WHERE h.user_id = u.id AND h.claimed_ms >= $2) AS beats
         FROM users u
         WHERE $1 = ''
            OR u.name ILIKE '%' || $1 || '%'
            OR u.slack_id = $1
            OR u.id::text = $1
         ORDER BY last_seen DESC NULLS LAST, u.id DESC
         LIMIT 50",
    )
    .bind(q)
    .bind(since)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| UserRow {
                    id: row.get("id"),
                    name: row.get("name"),
                    slack_id: row.get("slack_id"),
                    devices: row.get("devices"),
                    last_seen_ms: row.get("last_seen"),
                    credited_7d_ms: row.get("credited"),
                    heartbeats_7d: row.get("beats"),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => failed(e),
    }
}

// ---------------------------------------------------------------------------
// Timeline

#[derive(Deserialize)]
pub struct TimelineQuery {
    user: i64,
    from: i64,
    to: i64,
}

#[derive(Serialize)]
struct Timeline {
    user: UserInfo,
    from: i64,
    to: i64,
    devices: Vec<DeviceInfo>,
    heartbeats: Vec<HeartbeatRow>,
    flags: Vec<Flag>,
    summary: Summary,
}

#[derive(Serialize)]
struct UserInfo {
    id: i64,
    name: Option<String>,
    slack_id: Option<String>,
}

#[derive(Serialize)]
struct DeviceInfo {
    id: String,
    os: String,
}

#[derive(Serialize)]
struct HeartbeatRow {
    device: String,
    seq: i64,
    minute_ms: i64,
    received_ms: i64,
    offline: bool,
    stale: bool,
    session: String,
    agent: String,
    session_name: String,
    project: String,
    os: String,
    version: u32,
    agent_running: bool,
    input_kinds: String,
    slots: Vec<signals::Slot>,
    keys: Option<Vec<u16>>,
    mouse: Option<Vec<u16>>,
    injected: signals::Counts,
    prompts: signals::Prompts,
    responses: signals::Responses,
    interrupts: u32,
    files: Vec<FileRow>,
    client: String,
    models: Vec<String>,
    tools: Vec<signals::Tools>,
    multiplier: Option<f64>,
    credit_ms: i64,
    ai_ms: i64,
}

#[derive(Serialize)]
struct FileRow {
    path: String,
    wd_added: u32,
    wd_removed: u32,
    claimed_added: u32,
    claimed_removed: u32,
}

#[derive(Serialize, Default)]
struct Summary {
    heartbeats: usize,
    credited_ms: i64,
    agent_ms: i64,
    keys: u64,
    mouse: u64,
    injected: u64,
    prompts: u64,
    scripted_prompts: u64,
    offline: usize,
    stale: usize,
    flags: BTreeMap<&'static str, usize>,
}

/// The agent session id isn't in the review body; read it separately.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Ids {
    agent: String,
    agent_session_id: String,
}

pub async fn timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TimelineQuery>,
) -> Response {
    if let Err(r) = reviewer(&state, &headers).await {
        return *r;
    }
    if query.to <= query.from || query.to - query.from > MAX_RANGE_MS {
        return error(StatusCode::BAD_REQUEST, "range must be 1 ms to 7 days");
    }
    let user = match sqlx::query("SELECT id, name, slack_id FROM users WHERE id = $1")
        .bind(query.user)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(row)) => UserInfo {
            id: row.get("id"),
            name: row.get("name"),
            slack_id: row.get("slack_id"),
        },
        Ok(None) => return error(StatusCode::NOT_FOUND, "no such user"),
        Err(e) => return failed(e),
    };
    let rows = match sqlx::query(
        "SELECT h.device_id, h.seq, h.claimed_ms, h.received_ms, h.offline, h.stale,
                h.body::text AS body, m.multiplier, m.credit_ms, m.ai_ms
         FROM heartbeats h
         LEFT JOIN session_minutes m ON m.device_id = h.device_id AND m.seq = h.seq
         WHERE h.user_id = $1 AND h.claimed_ms >= $2 AND h.claimed_ms < $3
         ORDER BY h.claimed_ms, h.device_id, h.seq",
    )
    .bind(query.user)
    .bind(query.from)
    .bind(query.to)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => return failed(e),
    };

    let mut beats = Vec::with_capacity(rows.len());
    let mut extra = Vec::with_capacity(rows.len());
    for row in rows {
        let device: String = row.get("device_id");
        let text: String = row.get("body");
        let body: signals::Body = serde_json::from_str(&text).unwrap_or_default();
        let ids: Ids = serde_json::from_str(&text).unwrap_or_default();
        let session = credit::session_id(&device, &ids.agent, &ids.agent_session_id);
        let credit_ms: Option<i64> = row.get("credit_ms");
        beats.push(Beat {
            device: short(&device),
            seq: row.get("seq"),
            received_ms: row.get("received_ms"),
            offline: row.get("offline"),
            stale: row.get("stale"),
            session,
            credit_ms: credit_ms.unwrap_or(0),
            body,
        });
        extra.push((
            row.get::<Option<f64>, _>("multiplier"),
            row.get::<Option<i64>, _>("ai_ms").unwrap_or(0),
        ));
    }

    let flags = signals::flags(&beats);
    let mut summary = Summary {
        heartbeats: beats.len(),
        ..Summary::default()
    };
    for severity in [Severity::High, Severity::Medium, Severity::Low] {
        summary.flags.insert(
            match severity {
                Severity::High => "high",
                Severity::Medium => "medium",
                Severity::Low => "low",
            },
            flags.iter().filter(|f| f.severity == severity).count(),
        );
    }
    let mut devices: BTreeMap<String, String> = BTreeMap::new();
    // Agent time counts each minute once, however many sessions ran.
    let mut agent_minutes = HashSet::new();
    // Input is per device and minute, carried by every session's heartbeat.
    let mut input_counted = HashSet::new();
    let mut heartbeats = Vec::with_capacity(beats.len());
    for (beat, (multiplier, ai_ms)) in beats.into_iter().zip(extra) {
        let b = &beat.body;
        devices
            .entry(beat.device.clone())
            .and_modify(|os| {
                if !b.os.is_empty() {
                    os.clone_from(&b.os)
                }
            })
            .or_insert_with(|| b.os.clone());
        summary.credited_ms += beat.credit_ms;
        if ai_ms > 0 {
            agent_minutes.insert(b.minute_ms);
        }
        if input_counted.insert((beat.device.clone(), b.minute_ms)) {
            summary.keys += b.slots.iter().map(|s| u64::from(s.keys)).sum::<u64>();
            summary.mouse += b.slots.iter().map(|s| u64::from(s.mouse)).sum::<u64>();
            summary.injected += u64::from(b.injected.keys + b.injected.mouse);
        }
        summary.prompts += u64::from(b.prompts.submitted);
        summary.scripted_prompts += u64::from(b.prompts.scripted);
        summary.offline += usize::from(beat.offline);
        summary.stale += usize::from(beat.stale);
        let per_second = |v: &Vec<u16>| (v.len() == 60).then(|| v.clone());
        heartbeats.push(HeartbeatRow {
            keys: per_second(&b.seconds.keys),
            mouse: per_second(&b.seconds.mouse),
            files: b
                .files
                .iter()
                .map(|f| FileRow {
                    path: f.path.clone(),
                    wd_added: f.working_dir.added,
                    wd_removed: f.working_dir.removed,
                    claimed_added: f.agent_edits.added,
                    claimed_removed: f.agent_edits.removed,
                })
                .collect(),
            device: beat.device,
            seq: beat.seq,
            minute_ms: b.minute_ms,
            received_ms: beat.received_ms,
            offline: beat.offline,
            stale: beat.stale,
            session: beat.session,
            agent: b.agent.clone(),
            session_name: b.session_name.clone(),
            project: b.project.clone(),
            os: b.os.clone(),
            version: b.version,
            agent_running: b.agent_running,
            input_kinds: b.input_kinds.clone(),
            slots: b.slots.clone(),
            injected: b.injected,
            prompts: b.prompts.clone(),
            responses: b.responses.clone(),
            interrupts: b.interrupts,
            client: b.client.clone(),
            models: b.models.clone(),
            tools: b.tools.clone(),
            multiplier,
            credit_ms: beat.credit_ms,
            ai_ms,
        });
    }
    summary.agent_ms = agent_minutes.len() as i64 * 60_000;

    Json(Timeline {
        user,
        from: query.from,
        to: query.to,
        devices: devices
            .into_iter()
            .map(|(id, os)| DeviceInfo { id, os })
            .collect(),
        heartbeats,
        flags,
        summary,
    })
    .into_response()
}

/// A device key's first 8 hex characters: enough to tell a user's devices
/// apart on screen.
fn short(device: &str) -> String {
    device.chars().take(8).collect()
}
