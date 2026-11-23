//! The logged-in user's settings, edited on the website.
//!
//! - `GET /api/settings`: `{ "timezone": "Europe/Berlin" | null,
//!   "public_profile": true }`
//! - `POST /api/settings` with `{ "timezone": … }`: save the time zone. Only from our own
//!   pages (the `Origin` header must match `PUBLIC_URL`). The time zone must
//!   be one Postgres knows (IANA names).
//! - `POST /api/settings/profile` with `{ "public": bool }`: show or hide the
//!   public profile (`profile.rs`), same `Origin` check.

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

/// What `GET /api/settings` returns.
#[derive(Serialize)]
pub struct AllSettings {
    timezone: Option<String>,
    /// Whether the profile, its place on the leaderboard and its badges
    /// are public.
    public_profile: bool,
}

#[derive(Deserialize)]
pub struct ProfileSetting {
    public: bool,
}

fn failed(e: sqlx::Error) -> StatusCode {
    eprintln!("settings failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AllSettings>, StatusCode> {
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let row = sqlx::query("SELECT timezone, public_profile FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .map_err(failed)?;
    Ok(Json(AllSettings {
        timezone: row.get("timezone"),
        public_profile: row.get("public_profile"),
    }))
}

pub async fn save_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(setting): Json<ProfileSetting>,
) -> StatusCode {
    if !auth::from_this_site(&state, &headers) {
        return StatusCode::FORBIDDEN;
    }
    let Some(user) = auth::current_user(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED;
    };
    match sqlx::query("UPDATE users SET public_profile = $2 WHERE id = $1")
        .bind(user.id)
        .bind(setting.public)
        .execute(&state.db)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => failed(e),
    }
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
