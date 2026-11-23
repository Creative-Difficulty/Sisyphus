//! What the logged-in user's web dashboard shows. 401 without a session.
//!
//! - `GET /api/dashboard`: tracked time, agent time and counts for a date
//!   range and filters, per hour/day/week, and broken down by project,
//!   agent, operating system, language and task (Overview; `stats.rs`).
//! - `GET /api/projects`: every project with its tracked time (Projects).
//! - `GET /api/heartbeats?offset=N`: the heartbeats the user's devices
//!   uploaded, newest first, 100 at a time, exactly as stored (Heartbeats,
//!   a debug view).
//!
//! Tracked time is the credited time from `session_minutes` (see
//! `credit.rs`); "today" and "this week" follow the user's time zone (UTC
//! until set in the website's settings).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{AppState, auth};

#[derive(Serialize)]
pub struct Projects {
    projects: Vec<ProjectStat>,
}

/// One project, as the Projects view lists it.
#[derive(Serialize)]
struct ProjectStat {
    /// Project folder name.
    name: String,
    /// Credited time across all its sessions.
    tracked_ms: i64,
    sessions: i64,
    last_active_ms: i64,
}

pub async fn projects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Projects>, StatusCode> {
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let rows = sqlx::query(
        "SELECT s.project AS name,
                coalesce(sum(m.credit_ms), 0)::bigint AS tracked_ms,
                count(DISTINCT s.id) AS sessions,
                max(s.last_minute_ms) AS last_active_ms
         FROM agent_sessions s LEFT JOIN session_minutes m ON m.session_id = s.id
         WHERE s.user_id = $1
         GROUP BY s.project
         ORDER BY last_active_ms DESC",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(failed)?;
    Ok(Json(Projects {
        projects: rows
            .into_iter()
            .map(|row| ProjectStat {
                name: row.get("name"),
                tracked_ms: row.get("tracked_ms"),
                sessions: row.get("sessions"),
                last_active_ms: row.get("last_active_ms"),
            })
            .collect(),
    }))
}

pub fn failed(e: sqlx::Error) -> StatusCode {
    eprintln!("dashboard query failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Start of today and of this week (Monday) in the user's time zone
/// (unix ms). UTC until the user sets one.
pub async fn day_and_week_start(db: &sqlx::PgPool, user_id: i64) -> sqlx::Result<(i64, i64)> {
    let row = sqlx::query(
        "SELECT (extract(epoch FROM date_trunc('day', now() AT TIME ZONE tz) AT TIME ZONE tz)
                 * 1000)::bigint AS day,
                (extract(epoch FROM date_trunc('week', now() AT TIME ZONE tz) AT TIME ZONE tz)
                 * 1000)::bigint AS week
         FROM (SELECT coalesce(timezone, 'UTC') AS tz FROM users WHERE id = $1) u",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok((row.get("day"), row.get("week")))
}

/// Credited time and agent time of `user_id` since `since_ms`.
pub async fn credited_since(
    db: &sqlx::PgPool,
    user_id: i64,
    since_ms: i64,
) -> sqlx::Result<(i64, i64)> {
    let row = sqlx::query(
        "SELECT coalesce(sum(credit_ms), 0)::bigint AS credit,
                60000 * count(DISTINCT minute_ms) FILTER (WHERE ai_ms > 0) AS agent
         FROM session_minutes WHERE user_id = $1 AND minute_ms >= $2",
    )
    .bind(user_id)
    .bind(since_ms)
    .fetch_one(db)
    .await?;
    Ok((row.get("credit"), row.get("agent")))
}

/// Heartbeats per page of the debug view.
const HEARTBEATS_PAGE: i64 = 100;

#[derive(Deserialize)]
pub struct Page {
    #[serde(default)]
    offset: i64,
}

#[derive(Serialize)]
pub struct Heartbeats {
    /// How many the user's devices uploaded in total.
    total: i64,
    heartbeats: Vec<StoredHeartbeat>,
}

/// One heartbeat as the server stored it.
#[derive(Serialize)]
struct StoredHeartbeat {
    device_id: String,
    seq: i64,
    hash: String,
    prev: String,
    sig: String,
    /// The minute the client claims (untrusted).
    claimed_ms: i64,
    /// When the server received it.
    received_ms: i64,
    /// Received more than 5 minutes after its minute.
    offline: bool,
    /// What it was credited, once crediting ran.
    credit: Option<Credit>,
    /// The signed heartbeat body.
    body: serde_json::Value,
}

#[derive(Serialize)]
struct Credit {
    session_id: String,
    multiplier: f64,
    attention_end: f64,
    credit_ms: i64,
    ai_ms: i64,
    formula: i32,
}

pub async fn heartbeats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> Result<Json<Heartbeats>, StatusCode> {
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let failed = |e: sqlx::Error| {
        eprintln!("heartbeats failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let total: i64 = sqlx::query("SELECT count(*) AS n FROM heartbeats WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .map_err(failed)?
        .get("n");
    let rows = sqlx::query(
        "SELECT h.device_id, h.seq, h.hash, h.prev, h.sig, h.claimed_ms, h.received_ms,
                h.offline, h.body::text AS body, m.session_id, m.multiplier,
                m.attention_end, m.credit_ms, m.ai_ms, m.formula
         FROM heartbeats h
         LEFT JOIN session_minutes m ON m.device_id = h.device_id AND m.seq = h.seq
         WHERE h.user_id = $1
         ORDER BY h.received_ms DESC, h.claimed_ms DESC, h.seq DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(user.id)
    .bind(HEARTBEATS_PAGE)
    .bind(page.offset.max(0))
    .fetch_all(&state.db)
    .await
    .map_err(failed)?;
    let heartbeats = rows
        .into_iter()
        .map(|row| StoredHeartbeat {
            device_id: row.get("device_id"),
            seq: row.get("seq"),
            hash: row.get("hash"),
            prev: row.get("prev"),
            sig: row.get("sig"),
            claimed_ms: row.get("claimed_ms"),
            received_ms: row.get("received_ms"),
            offline: row.get("offline"),
            credit: row
                .get::<Option<String>, _>("session_id")
                .map(|session_id| Credit {
                    session_id,
                    multiplier: row.get("multiplier"),
                    attention_end: row.get("attention_end"),
                    credit_ms: row.get("credit_ms"),
                    ai_ms: row.get("ai_ms"),
                    formula: row.get("formula"),
                }),
            body: serde_json::from_str(&row.get::<String, _>("body"))
                .unwrap_or(serde_json::Value::Null),
        })
        .collect();
    Ok(Json(Heartbeats { total, heartbeats }))
}
