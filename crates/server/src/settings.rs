//! The logged-in user's settings, edited on the website.
//!
//! - `GET /api/settings`: `{ "timezone": "Europe/Berlin" | null }`
//! - `POST /api/settings` with the same JSON: save them. Only from our own
//!   pages (the `Origin` header must match `PUBLIC_URL`). The time zone must
//!   be one Postgres knows (IANA names).

use axum::Json;
use axum::extract::State;
use axum::http::header::ORIGIN;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{AppState, auth};

#[derive(Serialize, Deserialize)]
pub struct Settings {
    /// IANA time zone for "today" and "this week"; `None` means UTC.
    timezone: Option<String>,
}

fn failed(e: sqlx::Error) -> StatusCode {
    eprintln!("settings failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Settings>, StatusCode> {
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let timezone = sqlx::query("SELECT timezone FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .map_err(failed)?
        .get("timezone");
    Ok(Json(Settings { timezone }))
}

pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(settings): Json<Settings>,
) -> Result<Json<Settings>, StatusCode> {
    let same_origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin == state.public_origin());
    if !same_origin {
        return Err(StatusCode::FORBIDDEN);
    }
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if let Some(tz) = &settings.timezone {
        let known: bool =
            sqlx::query("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1) AS ok")
                .bind(tz)
                .fetch_one(&state.db)
                .await
                .map_err(failed)?
                .get("ok");
        if !known {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    sqlx::query("UPDATE users SET timezone = $2 WHERE id = $1")
        .bind(user.id)
        .bind(&settings.timezone)
        .execute(&state.db)
        .await
        .map_err(failed)?;
    Ok(Json(settings))
}
