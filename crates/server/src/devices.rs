//! The logged-in user's devices, on the website's Devices view.
//!
//! - `GET /api/devices`: every device linked to the account, most recently
//!   seen first, with its operating system (from its latest heartbeat that
//!   names one) and the time credited from its heartbeats.
//! - `POST /api/devices/unlink` with `{ "device_id": "<hex key>" }`: unlink
//!   it. Only from our own pages (the `Origin` header must match
//!   `PUBLIC_URL`). The device can no longer upload or make app requests
//!   until it logs in again; its heartbeats and credited time stay with the
//!   account (decided 2026-11-22).

use axum::Json;
use axum::extract::State;
use axum::http::header::ORIGIN;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{AppState, auth};

#[derive(Serialize)]
pub struct Devices {
    devices: Vec<Device>,
}

#[derive(Serialize)]
struct Device {
    /// The device's public key (hex).
    device_id: String,
    /// `windows`, `macos`, `linux` or `other`; `None` until it sends a
    /// heartbeat that names one (scheme v7).
    os: Option<String>,
    /// Credit from this device's heartbeats.
    tracked_ms: i64,
    first_seen_ms: i64,
    last_seen_ms: i64,
}

#[derive(Deserialize)]
pub struct Unlink {
    device_id: String,
}

fn failed(e: sqlx::Error) -> StatusCode {
    eprintln!("devices failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Devices>, StatusCode> {
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let rows = sqlx::query(
        "SELECT d.device_id, d.first_seen_ms, d.last_seen_ms,
                (SELECT h.body->>'os' FROM heartbeats h
                 WHERE h.device_id = d.device_id AND h.body ? 'os'
                 ORDER BY h.seq DESC LIMIT 1) AS os,
                (SELECT coalesce(sum(m.credit_ms), 0)::bigint FROM session_minutes m
                 WHERE m.device_id = d.device_id AND m.user_id = $1) AS tracked_ms
         FROM devices d
         WHERE d.user_id = $1
         ORDER BY d.last_seen_ms DESC",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(failed)?;
    Ok(Json(Devices {
        devices: rows
            .into_iter()
            .map(|row| Device {
                device_id: row.get("device_id"),
                os: row.get("os"),
                tracked_ms: row.get("tracked_ms"),
                first_seen_ms: row.get("first_seen_ms"),
                last_seen_ms: row.get("last_seen_ms"),
            })
            .collect(),
    }))
}

pub async fn unlink(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Unlink>,
) -> StatusCode {
    let same_origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin == state.public_origin());
    if !same_origin {
        return StatusCode::FORBIDDEN;
    }
    let Some(user) = auth::current_user(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED;
    };
    // Only the user's own devices; the heartbeats keep their owner.
    let result =
        sqlx::query("UPDATE devices SET user_id = NULL WHERE device_id = $1 AND user_id = $2")
            .bind(request.device_id.to_lowercase())
            .bind(user.id)
            .execute(&state.db)
            .await;
    match result {
        Ok(done) if done.rows_affected() == 1 => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => failed(e),
    }
}
