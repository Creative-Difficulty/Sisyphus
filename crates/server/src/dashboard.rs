//! What the logged-in user's web dashboard shows. 401 without a session.
//!
//! - `GET /api/dashboard`: tracked time and a few figures (Overview).
//! - `GET /api/projects`: every project with its tracked time (Projects).
//!
//! Tracked time and projects are placeholders (zero / empty) until the v2
//! heartbeat and server-side crediting exist; the response shapes are final,
//! so the page won't need to change. Only the device count is real.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
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
