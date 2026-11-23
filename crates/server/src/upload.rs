//! `POST /api/upload`: a batch of signed heartbeats from one device.
//!
//! Only devices linked to an account (see `auth.rs`) may upload; others get
//! 403. The batch must continue the device's hash chain exactly where the
//! stored chain tip ends (see `sisyphus_common::api`), and no heartbeat
//! may be dated in the future (its minute not over by the server's clock;
//! the whole batch is refused with `FutureDated`). On success the
//! heartbeats are stored with the server's receive time (flagged offline
//! when they arrive more than 5 minutes after their minute), credited (see
//! `credit.rs`) and the new tip is returned.
//!
//! Heartbeats must start on a whole minute (`Misaligned` otherwise). Ones
//! that arrive more than an hour after their minute, or are dated before
//! the device was linked, are stored as `stale` and earn nothing (decided
//! 2026-11-23). Crediting takes a per-user lock, so uploads from two of a
//! user's devices can't both keep a full share of the same minute.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sisyphus_common::api::{StoredHeartbeat, UploadAck, UploadBatch, UploadError, verify_batch};
use sisyphus_common::chain::ChainTip;
use sisyphus_common::hex::Hash;
use sqlx::{PgPool, Row};

use crate::{AppState, credit, now_ms};

pub async fn upload(State(state): State<AppState>, Json(batch): Json<UploadBatch>) -> Response {
    match store(&state.db, &batch).await {
        Ok(ack) => Json(ack).into_response(),
        Err(Rejected::Upload(e)) => {
            let status = match e {
                UploadError::OutOfSync { .. } => StatusCode::CONFLICT,
                UploadError::Empty
                | UploadError::Invalid(_)
                | UploadError::UnsupportedVersion { .. }
                | UploadError::FutureDated { .. }
                | UploadError::Misaligned { .. } => StatusCode::BAD_REQUEST,
            };
            (status, Json(e)).into_response()
        }
        Err(Rejected::NotLinked) => (
            StatusCode::FORBIDDEN,
            Json(ServerError {
                error: "device is not linked to an account",
            }),
        )
            .into_response(),
        Err(Rejected::Database(e)) => {
            eprintln!("upload failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ServerError {
                    error: "database error",
                }),
            )
                .into_response()
        }
    }
}

#[derive(Serialize)]
struct ServerError {
    error: &'static str,
}

enum Rejected {
    Upload(UploadError),
    NotLinked,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for Rejected {
    fn from(e: sqlx::Error) -> Self {
        Rejected::Database(e)
    }
}

async fn store(db: &PgPool, batch: &UploadBatch) -> Result<UploadAck, Rejected> {
    let now = now_ms();
    let device = batch.device.to_string();
    let mut tx = db.begin().await?;

    // Linking a device creates its row at the genesis tip. Locking the row
    // serialises concurrent uploads from the same device.
    let row = sqlx::query(
        "SELECT next_seq, prev, user_id, linked_ms FROM devices
         WHERE device_id = $1 AND user_id IS NOT NULL FOR UPDATE",
    )
    .bind(&device)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(Rejected::NotLinked)?;
    let tip = ChainTip {
        next_seq: row.get::<i64, _>("next_seq") as u64,
        prev: parse_hash(&row.get::<String, _>("prev")),
    };

    let user_id: i64 = row.get("user_id");
    let linked_ms: i64 = row.get("linked_ms");
    let new_tip = verify_batch(batch, &tip, now as u64).map_err(Rejected::Upload)?;
    // Minutes are shared among all of the user's sessions and devices
    // (`credit::share_minute`), which only works if no other upload of the
    // same user credits at the same time.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let mut minutes = std::collections::BTreeSet::new();

    for signed in &batch.heartbeats {
        let body = serde_json::to_string(&signed.heartbeat).expect("heartbeat serializes");
        let stored = StoredHeartbeat {
            signed: signed.clone(),
            received_at_ms: now as u64,
        };
        let offline = stored.is_offline();
        let stale = stored.is_stale() || (signed.heartbeat.minute_ms as i64) < linked_ms;
        sqlx::query(
            "INSERT INTO heartbeats
                 (device_id, seq, hash, prev, sig, claimed_ms, received_ms, body, offline,
                  user_id, stale)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $10, $11)",
        )
        .bind(&device)
        .bind(signed.seq as i64)
        .bind(signed.hash.to_string())
        .bind(signed.prev.to_string())
        .bind(signed.sig.to_string())
        .bind(signed.heartbeat.minute_ms as i64)
        .bind(now)
        .bind(&body)
        .bind(offline)
        .bind(user_id)
        .bind(stale)
        .execute(&mut *tx)
        .await?;
        let stored = credit::Stored {
            user_id,
            device_id: &device,
            seq: signed.seq as i64,
            offline,
            stale,
            body: &body,
        };
        if let Some(minute) = credit::process(&mut tx, stored).await? {
            minutes.insert(minute);
        }
    }
    for minute in minutes {
        credit::share_minute(&mut tx, user_id, minute).await?;
    }
    sqlx::query(
        "UPDATE devices SET next_seq = $2, prev = $3, last_seen_ms = $4 WHERE device_id = $1",
    )
    .bind(&device)
    .bind(new_tip.next_seq as i64)
    .bind(new_tip.prev.to_string())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(UploadAck {
        tip: new_tip,
        received_at_ms: now as u64,
    })
}

/// Hashes are stored as the same hex text they serialize to.
fn parse_hash(hex: &str) -> Hash {
    serde_json::from_value(serde_json::Value::String(hex.to_string()))
        .expect("stored hashes are valid hex")
}
