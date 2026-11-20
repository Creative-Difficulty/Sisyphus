//! `GET /api/dashboard`: what the web page shows. Read-only, no accounts yet.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;
use sisyphus_common::api::OFFLINE_THRESHOLD_MS;
use sqlx::{PgPool, Row};

use crate::now_ms;

/// How many recent heartbeats the page lists.
const RECENT: i64 = 50;

#[derive(Serialize)]
pub struct Dashboard {
    server_time_ms: i64,
    total_heartbeats: i64,
    devices: Vec<Device>,
    recent: Vec<Recent>,
}

#[derive(Serialize)]
struct Device {
    /// First 8 hex characters of the device key; enough to tell devices apart.
    short_id: String,
    heartbeats: i64,
    last_seen_ms: i64,
}

#[derive(Serialize)]
struct Recent {
    device: String,
    app: String,
    project: Option<String>,
    received_ms: i64,
    /// Arrived long after its claimed time, so it was likely recorded offline.
    offline: bool,
}

pub async fn dashboard(State(db): State<PgPool>) -> Result<Json<Dashboard>, StatusCode> {
    load(&db).await.map(Json).map_err(|e| {
        eprintln!("dashboard failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

async fn load(db: &PgPool) -> Result<Dashboard, sqlx::Error> {
    let total_heartbeats: i64 = sqlx::query("SELECT count(*) AS n FROM heartbeats")
        .fetch_one(db)
        .await?
        .get("n");

    let devices = sqlx::query(
        "SELECT d.device_id, d.last_seen_ms, count(h.seq) AS heartbeats
         FROM devices d LEFT JOIN heartbeats h ON h.device_id = d.device_id
         GROUP BY d.device_id ORDER BY d.last_seen_ms DESC",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|row| Device {
        short_id: short(&row.get::<String, _>("device_id")),
        heartbeats: row.get("heartbeats"),
        last_seen_ms: row.get("last_seen_ms"),
    })
    .collect();

    let recent = sqlx::query(
        "SELECT device_id, claimed_ms, received_ms,
                body->>'app' AS app, body->>'project' AS project
         FROM heartbeats ORDER BY received_ms DESC, seq DESC LIMIT $1",
    )
    .bind(RECENT)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|row| {
        let claimed: i64 = row.get("claimed_ms");
        let received: i64 = row.get("received_ms");
        Recent {
            device: short(&row.get::<String, _>("device_id")),
            app: row.get::<Option<String>, _>("app").unwrap_or_default(),
            project: row.get("project"),
            received_ms: received,
            offline: received.saturating_sub(claimed) > OFFLINE_THRESHOLD_MS as i64,
        }
    })
    .collect();

    Ok(Dashboard {
        server_time_ms: now_ms(),
        total_heartbeats,
        devices,
        recent,
    })
}

fn short(device_id: &str) -> String {
    device_id.chars().take(8).collect()
}
