//! Public profiles, the leaderboard and README badges (decided 2026-11-23).
//!
//! Profiles are public by default; a user turns theirs off in the website's
//! settings (`users.public_profile`, `settings.rs`). A private profile is
//! visible only to its owner, is left off the leaderboard, and its badges
//! say "not public".
//!
//! - `GET /api/u/{id}`: name, whether the profile is public, whether it's
//!   the viewer's own, and each project with its tracked time and its
//!   repository and live demo links. 404 when private (unless it's the
//!   viewer's own) or unknown.
//! - `GET /api/u/{id}/dashboard?range=…&project=…`: the same stats as the
//!   Overview (`stats.rs`), in the owner's time zone, without file paths.
//! - `GET /api/leaderboard?range=…`: public profiles ranked by tracked time
//!   (`today`, `week`, `30d`, `all`; in the viewer's time zone, UTC when
//!   logged out or unset), at most [`LEADERBOARD`].
//! - `GET /badge/{id}/{project}.svg?style=…`: a project's all-time tracked
//!   time as a badge for its README (`flat`, `flat-square`,
//!   `for-the-badge` or `pink`).
//!
//! Never public: session titles, file paths, devices, heartbeats.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::dashboard::failed;
use crate::{AppState, auth, stats};

/// Users listed on the leaderboard.
const LEADERBOARD: i64 = 100;

#[derive(Serialize)]
pub struct Profile {
    id: i64,
    name: Option<String>,
    public: bool,
    /// The viewer's own profile.
    own: bool,
    projects: Vec<PublicProject>,
}

#[derive(Serialize)]
struct PublicProject {
    name: String,
    tracked_ms: i64,
    repo_url: Option<String>,
    demo_url: Option<String>,
}

/// The profile's owner if the viewer may see it: it's public or theirs.
async fn visible(
    state: &AppState,
    headers: &HeaderMap,
    id: i64,
) -> Result<(Option<String>, bool, bool), StatusCode> {
    let row = sqlx::query("SELECT name, public_profile FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(failed)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let public: bool = row.get("public_profile");
    let own = auth::current_user(state, headers)
        .await
        .is_some_and(|user| user.id == id);
    if !public && !own {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok((row.get("name"), public, own))
}

pub async fn profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Profile>, StatusCode> {
    let (name, public, own) = visible(&state, &headers, id).await?;
    let rows = sqlx::query(
        "SELECT s.project AS name, sum(m.credit_ms)::bigint AS tracked_ms,
                max(s.last_minute_ms) AS last_active_ms, p.repo_url, p.demo_url
         FROM agent_sessions s
              JOIN session_minutes m ON m.session_id = s.id
              LEFT JOIN projects p ON p.user_id = s.user_id AND p.name = s.project
         WHERE s.user_id = $1
         GROUP BY s.project, p.repo_url, p.demo_url
         HAVING sum(m.credit_ms) > 0
         ORDER BY tracked_ms DESC, name",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(failed)?;
    Ok(Json(Profile {
        id,
        name,
        public,
        own,
        projects: rows
            .into_iter()
            .map(|row| PublicProject {
                name: row.get("name"),
                tracked_ms: row.get("tracked_ms"),
                repo_url: row.get("repo_url"),
                demo_url: row.get("demo_url"),
            })
            .collect(),
    }))
}

pub async fn profile_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(filters): Query<stats::Filters>,
) -> Result<Json<stats::Stats>, StatusCode> {
    visible(&state, &headers, id).await?;
    stats::compute(&state.db, id, &filters, false)
        .await
        .map(Json)
        .map_err(failed)
}

#[derive(Deserialize)]
pub struct Range {
    range: Option<String>,
}

#[derive(Serialize)]
pub struct Leaderboard {
    range: &'static str,
    /// The time zone "today" and "this week" follow.
    timezone: String,
    users: Vec<Ranked>,
}

#[derive(Serialize)]
struct Ranked {
    rank: usize,
    id: i64,
    name: Option<String>,
    tracked_ms: i64,
    agent_ms: i64,
    /// The viewer.
    me: bool,
}

pub async fn leaderboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<Range>,
) -> Result<Json<Leaderboard>, StatusCode> {
    let viewer = auth::current_user(&state, &headers).await.map(|u| u.id);
    let range = match query.range.as_deref() {
        Some("today") => "today",
        Some("30d") => "30d",
        Some("all") => "all",
        _ => "week",
    };
    let rows = sqlx::query(
        "WITH v AS (
             SELECT coalesce((SELECT timezone FROM users WHERE id = $1), 'UTC') AS tz),
         start AS (
             SELECT tz, CASE $2
                 WHEN 'today' THEN (extract(epoch FROM date_trunc('day', now() AT TIME ZONE tz)
                                    AT TIME ZONE tz) * 1000)::bigint
                 WHEN 'week' THEN (extract(epoch FROM date_trunc('week', now() AT TIME ZONE tz)
                                   AT TIME ZONE tz) * 1000)::bigint
                 WHEN '30d' THEN (extract(epoch FROM (date_trunc('day', now() AT TIME ZONE tz)
                                  - interval '29 days') AT TIME ZONE tz) * 1000)::bigint
                 ELSE 0 END AS ms
             FROM v)
         SELECT start.tz, u.id, u.name,
                sum(m.credit_ms)::bigint AS tracked_ms,
                60000 * count(DISTINCT m.minute_ms) FILTER (WHERE m.ai_ms > 0) AS agent_ms
         FROM start, session_minutes m JOIN users u ON u.id = m.user_id
         WHERE u.public_profile AND m.minute_ms >= start.ms
         GROUP BY start.tz, u.id, u.name
         HAVING sum(m.credit_ms) > 0
         ORDER BY tracked_ms DESC, u.id
         LIMIT $3",
    )
    .bind(viewer)
    .bind(range)
    .bind(LEADERBOARD)
    .fetch_all(&state.db)
    .await
    .map_err(failed)?;
    let timezone = match rows.first() {
        Some(row) => row.get("tz"),
        None => {
            sqlx::query("SELECT coalesce((SELECT timezone FROM users WHERE id = $1), 'UTC') AS tz")
                .bind(viewer)
                .fetch_one(&state.db)
                .await
                .map_err(failed)?
                .get("tz")
        }
    };
    let users = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let id: i64 = row.get("id");
            Ranked {
                rank: i + 1,
                id,
                name: row.get("name"),
                tracked_ms: row.get("tracked_ms"),
                agent_ms: row.get("agent_ms"),
                me: viewer == Some(id),
            }
        })
        .collect();
    Ok(Json(Leaderboard {
        range,
        timezone,
        users,
    }))
}

#[derive(Deserialize)]
pub struct BadgeQuery {
    style: Option<String>,
}

/// `GET /badge/{id}/{project}.svg`: the project's all-time tracked time,
/// or "not public" when the profile is private or has no such project.
pub async fn badge(
    State(state): State<AppState>,
    Path((id, file)): Path<(i64, String)>,
    Query(query): Query<BadgeQuery>,
) -> Response {
    let style = Style::parse(query.style.as_deref());
    let Some(project) = file.strip_suffix(".svg") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let tracked: Option<i64> = sqlx::query(
        "SELECT sum(m.credit_ms)::bigint AS tracked_ms
         FROM users u
              JOIN agent_sessions s ON s.user_id = u.id
              JOIN session_minutes m ON m.session_id = s.id
         WHERE u.id = $1 AND u.public_profile AND s.project = $2",
    )
    .bind(id)
    .bind(project)
    .fetch_one(&state.db)
    .await
    .map(|row| row.get("tracked_ms"))
    .unwrap_or_else(|e| {
        eprintln!("badge query failed: {e}");
        None
    });
    let (value, color) = match tracked {
        Some(ms) => (duration(ms), PINK),
        None => ("not public".to_string(), GREY),
    };
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            // GitHub's image proxy follows this, so badges stay fresh.
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        render(style, "Sisyphus", &value, color),
    )
        .into_response()
}

/// "12h 05m", like the website.
fn duration(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

const PLUM: &str = "#4a1942";
const PINK: &str = "#e02a78";
const GREY: &str = "#8f7389";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Flat,
    FlatSquare,
    ForTheBadge,
    Pink,
}

impl Style {
    fn parse(value: Option<&str>) -> Style {
        match value {
            Some("flat-square") => Style::FlatSquare,
            Some("for-the-badge") => Style::ForTheBadge,
            Some("pink") => Style::Pink,
            _ => Style::Flat,
        }
    }
}

/// Approximate width of `text` in Verdana at 11px, which badges are drawn
/// in (the font every badge renderer assumes; close enough for padding).
fn text_width(text: &str) -> f64 {
    text.chars()
        .map(|c| match c {
            'i' | 'l' | '.' | ',' | ':' | ';' | '\'' | '|' | '!' => 3.6,
            'f' | 'j' | 'r' | 't' | 'I' | '(' | ')' | '[' | ']' | '/' | '-' => 4.6,
            ' ' => 3.9,
            'm' | 'w' => 10.7,
            'M' | 'W' => 11.5,
            '0'..='9' => 7.0,
            'A'..='Z' => 7.6,
            _ => 6.8,
        })
        .sum()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// A two-part badge: `label` on plum, `value` on `color`.
pub fn render(style: Style, label: &str, value: &str, color: &str) -> String {
    let title = escape(&format!("{label}: {value}"));
    let font = "Verdana,Geneva,DejaVu Sans,sans-serif";
    if style == Style::ForTheBadge {
        let (label, value) = (label.to_uppercase(), value.to_uppercase());
        // Bold 10px with 1.25px letter spacing.
        let width = |t: &str| (text_width(t) * 0.98 + 1.25 * t.chars().count() as f64).round();
        let lw = width(&label) + 24.0;
        let rw = width(&value) + 24.0;
        let w = lw + rw;
        return format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="28" role="img" aria-label="{title}"><title>{title}</title><rect width="{lw}" height="28" fill="{PLUM}"/><rect x="{lw}" width="{rw}" height="28" fill="{color}"/><g fill="#fff" text-anchor="middle" font-family="{font}" font-size="10" font-weight="bold" letter-spacing="1.25"><text x="{lx}" y="17.5">{label}</text><text x="{rx}" y="17.5">{value}</text></g></svg>"##,
            lx = lw / 2.0,
            rx = lw + rw / 2.0,
            label = escape(&label),
            value = escape(&value),
        );
    }
    let (height, radius, pad, baseline) = match style {
        Style::Pink => (22.0, 11.0, 12.0, 15.0),
        Style::FlatSquare => (20.0, 0.0, 6.0, 14.0),
        _ => (20.0, 3.0, 6.0, 14.0),
    };
    let lw = (text_width(label) + 2.0 * pad).round();
    let rw = (text_width(value) + 2.0 * pad).round();
    let w = lw + rw;
    let (lx, rx) = (lw / 2.0, lw + rw / 2.0);
    // Flat badges get a faint gradient and a text shadow, like most badges.
    let flat = style == Style::Flat;
    let gradient = if flat {
        r##"<linearGradient id="s" x2="0" y2="100%"><stop offset="0" stop-color="#bbb" stop-opacity=".1"/><stop offset="1" stop-opacity=".1"/></linearGradient>"##
    } else {
        ""
    };
    let shade = if flat {
        format!(r##"<rect width="{w}" height="{height}" fill="url(#s)"/>"##)
    } else {
        String::new()
    };
    let shadow = |x: f64, text: &str| {
        if flat {
            format!(
                r##"<text x="{x}" y="{}" fill="#010101" fill-opacity=".3">{text}</text>"##,
                baseline + 1.0
            )
        } else {
            String::new()
        }
    };
    let (label, value) = (escape(label), escape(value));
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{height}" role="img" aria-label="{title}"><title>{title}</title>{gradient}<clipPath id="r"><rect width="{w}" height="{height}" rx="{radius}" fill="#fff"/></clipPath><g clip-path="url(#r)"><rect width="{lw}" height="{height}" fill="{PLUM}"/><rect x="{lw}" width="{rw}" height="{height}" fill="{color}"/>{shade}</g><g fill="#fff" text-anchor="middle" font-family="{font}" font-size="11">{ls}<text x="{lx}" y="{baseline}">{label}</text>{rs}<text x="{rx}" y="{baseline}">{value}</text></g></svg>"##,
        ls = shadow(lx, &label),
        rs = shadow(rx, &value),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles() {
        assert_eq!(Style::parse(None), Style::Flat);
        assert_eq!(Style::parse(Some("nope")), Style::Flat);
        assert_eq!(Style::parse(Some("pink")), Style::Pink);
        assert_eq!(Style::parse(Some("for-the-badge")), Style::ForTheBadge);
    }

    #[test]
    fn badges_escape_their_text() {
        for style in [
            Style::Flat,
            Style::FlatSquare,
            Style::ForTheBadge,
            Style::Pink,
        ] {
            let svg = render(style, "Sisyphus", "<a href=\"x\">&", PINK);
            assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
            assert!(!svg.contains("<a "), "{svg}");
            assert!(svg.contains("&lt;"), "{svg}");
        }
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0h 00m");
        assert_eq!(duration(3_660_000 + 59_999), "1h 01m");
        assert_eq!(duration(-5), "0h 00m");
    }
}
