//! What the logged-in user's web dashboard shows. 401 without a session.
//!
//! - `GET /api/dashboard`: tracked time and a few figures (Overview).
//! - `GET /api/projects`: every project with its tracked time (Projects).
//! - `GET /api/heartbeats?offset=N`: the heartbeats the user's devices
//!   uploaded, newest first, 100 at a time, exactly as stored (Heartbeats,
//!   a debug view).
//!
//! Tracked time and projects are placeholders (zero / empty) until the v2
//! heartbeat and server-side crediting exist; the response shapes are final,
//! so the page won't need to change. Only the device count is real.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{AppState, auth};

#[derive(Serialize)]
pub struct Dashboard {
    tracked_today_ms: i64,
    tracked_week_ms: i64,
    tracked_total_ms: i64,
    projects: i64,
    sessions: i64,
    devices: i64,
}

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

pub async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Dashboard>, StatusCode> {
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let devices: i64 = sqlx::query("SELECT count(*) AS n FROM devices WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            eprintln!("dashboard failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .get("n");
    Ok(Json(Dashboard {
        // Placeholders until the server credits time.
        tracked_today_ms: 0,
        tracked_week_ms: 0,
        tracked_total_ms: 0,
        projects: 0,
        sessions: 0,
        devices,
    }))
}

pub async fn projects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Projects>, StatusCode> {
    auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    // Placeholder until heartbeats carry sessions and the server credits time.
    Ok(Json(Projects {
        projects: Vec::new(),
    }))
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
    /// The signed heartbeat body.
    body: serde_json::Value,
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
    let total: i64 = sqlx::query(
        "SELECT count(*) AS n FROM heartbeats h
         JOIN devices d ON d.device_id = h.device_id WHERE d.user_id = $1",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await
    .map_err(failed)?
    .get("n");
    let rows = sqlx::query(
        "SELECT h.device_id, h.seq, h.hash, h.prev, h.sig, h.claimed_ms, h.received_ms,
                h.body::text AS body
         FROM heartbeats h JOIN devices d ON d.device_id = h.device_id
         WHERE d.user_id = $1
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
            body: serde_json::from_str(&row.get::<String, _>("body"))
                .unwrap_or(serde_json::Value::Null),
        })
        .collect();
    Ok(Json(Heartbeats { total, heartbeats }))
}
