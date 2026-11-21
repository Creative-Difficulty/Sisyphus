//! Requests from the desktop app, signed with its device key (see
//! `sisyphus_common::api::request_message`). Only devices linked to an
//! account get an answer:
//!
//! - `GET /api/app/today?start_ms=…`: credited time and agent time today, in the time zone
//!   set on the website, or since `start_ms` (the app's local midnight)
//!   while none is set.
//! - `POST /api/app/logout`: unlink this device from its account. It can't
//!   upload or read anything until it logs in again.
//!
//! A request is refused (401) unless its signature is valid, it was signed
//! within [`REQUEST_MAX_SKEW_MS`] of the server's clock and the same
//! signature wasn't seen before; 403 if the device isn't linked.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sisyphus_common::api::{
    HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME, REQUEST_MAX_SKEW_MS, Today, request_message,
};
use sqlx::Row;

use crate::{AppState, now_ms};

/// Signatures seen within the allowed clock skew, so a captured request
/// can't be sent again. Kept in memory: a restart forgets them, but they'd
/// expire within minutes anyway.
#[derive(Default)]
pub struct SeenRequests(Mutex<HashMap<[u8; 64], u64>>);

impl SeenRequests {
    /// Remember `signature` until `expires_ms`. False if it was seen.
    fn first_time(&self, signature: [u8; 64], now: u64) -> bool {
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() > 10_000 {
            seen.retain(|_, expires| *expires > now);
        }
        match seen.get(&signature) {
            Some(expires) if *expires > now => false,
            _ => {
                seen.insert(signature, now + 2 * REQUEST_MAX_SKEW_MS);
                true
            }
        }
    }
}

/// A device that proved itself with a signed request and is linked to an
/// account.
pub struct LinkedDevice {
    pub device_id: String,
    pub user_id: i64,
}

/// Check a signed app request. 401 for a missing, stale, replayed or
/// invalid signature; 403 if the device isn't linked to an account.
pub async fn verify(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<LinkedDevice, StatusCode> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)
    };
    let device_id = header(HEADER_DEVICE)?.to_ascii_lowercase();
    let time_ms: u64 = header(HEADER_TIME)?
        .parse()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let signature: [u8; 64] = hex::decode(header(HEADER_SIGNATURE)?)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let key: [u8; 32] = hex::decode(&device_id)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| StatusCode::UNAUTHORIZED)?;

    let now = now_ms() as u64;
    if now.abs_diff(time_ms) > REQUEST_MAX_SKEW_MS {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let path = uri.path_and_query().map_or(uri.path(), |p| p.as_str());
    let message = request_message(method.as_str(), path, time_ms, body);
    key.verify(&message, &Signature::from_bytes(&signature))
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    // Only after the signature checks out, so nobody can fill the cache
    // with junk.
    if !state.seen_requests.first_time(signature, now) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let user_id: Option<i64> = sqlx::query("SELECT user_id FROM devices WHERE device_id = $1")
        .bind(&device_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            eprintln!("device lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .and_then(|row| row.get("user_id"));
    let user_id = user_id.ok_or(StatusCode::FORBIDDEN)?;
    Ok(LinkedDevice { device_id, user_id })
}

#[derive(Deserialize)]
pub struct TodayParams {
    start_ms: u64,
}

pub async fn today(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<TodayParams>,
) -> Result<Json<Today>, StatusCode> {
    let device = verify(&state, &method, &uri, &headers, &[]).await?;
    let failed = |e: sqlx::Error| {
        eprintln!("today failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    };
    // The time zone set on the website wins; otherwise the app's own day.
    let has_timezone: bool =
        sqlx::query("SELECT timezone IS NOT NULL AS set FROM users WHERE id = $1")
            .bind(device.user_id)
            .fetch_one(&state.db)
            .await
            .map_err(failed)?
            .get("set");
    let start = if has_timezone {
        crate::dashboard::day_and_week_start(&state.db, device.user_id)
            .await
            .map_err(failed)?
            .0
    } else {
        params.start_ms.min(i64::MAX as u64) as i64
    };
    let (tracked, agent) = crate::dashboard::credited_since(&state.db, device.user_id, start)
        .await
        .map_err(failed)?;
    Ok(Json(Today {
        tracked_ms: tracked.max(0) as u64,
        agent_ms: agent.max(0) as u64,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let device = verify(&state, &method, &uri, &headers, &body).await?;
    sqlx::query("UPDATE devices SET user_id = NULL WHERE device_id = $1 AND user_id = $2")
        .bind(&device.device_id)
        .bind(device.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            eprintln!("device logout failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_is_accepted_once() {
        let seen = SeenRequests::default();
        assert!(seen.first_time([1; 64], 1_000));
        assert!(!seen.first_time([1; 64], 2_000));
        assert!(seen.first_time([2; 64], 2_000));
        // Long after it would have been refused for its time anyway.
        assert!(seen.first_time([1; 64], 1_000 + 3 * REQUEST_MAX_SKEW_MS));
    }
}
