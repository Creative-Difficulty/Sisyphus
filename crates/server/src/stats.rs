//! `GET /api/dashboard`: the Overview's stats (decided 2026-11-23).
//!
//! Query: `range` (`today`, `week` from Monday, `30d` with today, `all`;
//! default `today`, in the user's time zone) and optional filters
//! `project`, `agent`, `os`, `language` and `task`. An empty filter value
//! selects minutes without one: an unknown system, no file changes, no
//! scored prompt yet.
//!
//! Everything is credited time (`session_minutes.credit_ms`) except agent
//! time, where a minute with several agents running counts once.
//! - Languages: a minute's credit is split across the languages of the
//!   files that changed in it, in proportion to the characters changed
//!   (`minute_languages`). With a language filter, only that share counts.
//! - Tasks: each minute counts for the top task of its session's latest
//!   scored prompt (`session_minutes.task`).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::PgRow;

use crate::dashboard::failed;
use crate::{AppState, auth};

#[derive(Deserialize)]
pub struct Filters {
    range: Option<String>,
    project: Option<String>,
    agent: Option<String>,
    os: Option<String>,
    language: Option<String>,
    task: Option<String>,
}

#[derive(Serialize)]
pub struct Stats {
    range: &'static str,
    /// The time zone the buckets follow (IANA name; UTC until set).
    timezone: String,
    tracked_ms: i64,
    agent_ms: i64,
    sessions: i64,
    projects: i64,
    /// `hour`, `day` or `week`.
    unit: &'static str,
    /// One per hour/day/week from the range's start to now, empty ones
    /// included.
    buckets: Vec<Bucket>,
    by_project: Vec<Item>,
    by_agent: Vec<Item>,
    by_os: Vec<Item>,
    by_language: Vec<Item>,
    by_task: Vec<Item>,
    /// Every value the filters can take (all time, not just this range).
    options: Options,
}

#[derive(Serialize)]
struct Bucket {
    start_ms: i64,
    tracked_ms: i64,
    agent_ms: i64,
}

#[derive(Serialize)]
struct Item {
    /// The raw value (agent id, `os`, language name, task id); empty for
    /// none.
    name: String,
    tracked_ms: i64,
}

#[derive(Serialize)]
struct Options {
    projects: Vec<String>,
    agents: Vec<String>,
    os: Vec<String>,
    languages: Vec<String>,
    tasks: Vec<String>,
}

/// The user's minutes in the range that pass the filters, each with its
/// weight: 1, or with a language filter that language's share of the
/// minute's changes. $1 user, $2 range start (ms), $3 project, $4 agent,
/// $5 os, $6 language, $7 task.
const BASE: &str = "
WITH u AS (SELECT coalesce(timezone, 'UTC') AS tz FROM users WHERE id = $1),
weighted AS (
    SELECT m.session_id, m.minute_ms, m.credit_ms, m.ai_ms, m.os, m.task, s.project, s.agent,
           CASE
               WHEN $6::text IS NULL THEN 1.0::float8
               WHEN $6 = '' THEN CASE WHEN EXISTS (
                   SELECT 1 FROM minute_languages l
                   WHERE l.session_id = m.session_id AND l.minute_ms = m.minute_ms)
                   THEN 0.0::float8 ELSE 1.0::float8 END
               ELSE coalesce((
                   SELECT (sum(l.chars) FILTER (WHERE l.language = $6))::float8 / sum(l.chars)
                   FROM minute_languages l
                   WHERE l.session_id = m.session_id AND l.minute_ms = m.minute_ms), 0.0)
           END AS weight
    FROM session_minutes m JOIN agent_sessions s ON s.id = m.session_id
    WHERE m.user_id = $1 AND m.minute_ms >= $2
      AND ($3::text IS NULL OR s.project = $3)
      AND ($4::text IS NULL OR s.agent = $4)
      AND ($5::text IS NULL OR m.os = $5)
      AND ($7::text IS NULL OR m.task = $7)
),
base AS (SELECT * FROM weighted WHERE weight > 0)
";

pub async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(filters): Query<Filters>,
) -> Result<Json<Stats>, StatusCode> {
    let user = auth::current_user(&state, &headers)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let db = &state.db;
    let starts = sqlx::query(
        "SELECT (extract(epoch FROM date_trunc('day', now() AT TIME ZONE tz) AT TIME ZONE tz)
                 * 1000)::bigint AS day,
                (extract(epoch FROM date_trunc('week', now() AT TIME ZONE tz) AT TIME ZONE tz)
                 * 1000)::bigint AS week,
                (extract(epoch FROM (date_trunc('day', now() AT TIME ZONE tz) - interval '29 days')
                 AT TIME ZONE tz) * 1000)::bigint AS days30,
                (extract(epoch FROM now()) * 1000)::bigint AS now,
                (SELECT min(minute_ms) FROM session_minutes WHERE user_id = $1) AS first,
                tz
         FROM (SELECT coalesce(timezone, 'UTC') AS tz FROM users WHERE id = $1) u",
    )
    .bind(user.id)
    .fetch_one(db)
    .await
    .map_err(failed)?;
    let now: i64 = starts.get("now");
    let first: Option<i64> = starts.get("first");
    // Where the range starts (for the query) and where its bars start.
    let (range, start, series_start, unit) = match filters.range.as_deref() {
        Some("week") => {
            let week: i64 = starts.get("week");
            ("week", week, week, "day")
        }
        Some("30d") => {
            let days30: i64 = starts.get("days30");
            ("30d", days30, days30, "day")
        }
        Some("all") => {
            let first = first.unwrap_or(now);
            let unit = if now - first > 92 * 86_400_000 {
                "week"
            } else {
                "day"
            };
            ("all", 0, first, unit)
        }
        _ => {
            let day: i64 = starts.get("day");
            ("today", day, day, "hour")
        }
    };

    macro_rules! query {
        ($sql:expr) => {
            // Built only from the constants in this file; values are bound.
            sqlx::query(sqlx::AssertSqlSafe($sql))
                .bind(user.id)
                .bind(start)
                .bind(filters.project.as_deref())
                .bind(filters.agent.as_deref())
                .bind(filters.os.as_deref())
                .bind(filters.language.as_deref())
                .bind(filters.task.as_deref())
        };
    }

    let totals_sql = format!(
        "{BASE}
         SELECT coalesce(sum(credit_ms * weight), 0)::float8 AS tracked,
                60000 * coalesce((SELECT sum(w) FROM (
                    SELECT max(weight) AS w FROM base WHERE ai_ms > 0 GROUP BY minute_ms) a), 0)::float8
                    AS agent,
                count(DISTINCT session_id) AS sessions,
                count(DISTINCT project) AS projects
         FROM base"
    );
    let totals = query!(totals_sql).fetch_one(db).await.map_err(failed)?;

    let buckets_sql = format!(
        "{BASE},
         series AS (
             SELECT generate_series(date_trunc($8, to_timestamp($9::bigint / 1000.0) AT TIME ZONE u.tz),
                                    now() AT TIME ZONE u.tz, ('1 ' || $8)::interval) AS b
             FROM u),
         credit AS (
             SELECT date_trunc($8, to_timestamp(minute_ms / 1000.0) AT TIME ZONE u.tz) AS b,
                    sum(credit_ms * weight) AS tracked
             FROM base, u GROUP BY 1),
         ai AS (
             SELECT b, sum(w) AS agent FROM (
                 SELECT date_trunc($8, to_timestamp(minute_ms / 1000.0) AT TIME ZONE u.tz) AS b,
                        max(weight) AS w
                 FROM base, u WHERE ai_ms > 0 GROUP BY 1, minute_ms) x
             GROUP BY b)
         SELECT (extract(epoch FROM series.b AT TIME ZONE u.tz) * 1000)::bigint AS start_ms,
                coalesce(credit.tracked, 0)::float8 AS tracked,
                60000 * coalesce(ai.agent, 0)::float8 AS agent
         FROM series CROSS JOIN u
              LEFT JOIN credit ON credit.b = series.b
              LEFT JOIN ai ON ai.b = series.b
         ORDER BY series.b"
    );
    let buckets = query!(buckets_sql)
        .bind(unit)
        .bind(series_start)
        .fetch_all(db)
        .await
        .map_err(failed)?
        .iter()
        .map(|row| Bucket {
            start_ms: row.get("start_ms"),
            tracked_ms: ms(row, "tracked"),
            agent_ms: ms(row, "agent"),
        })
        .collect();

    let mut by = Vec::new();
    for column in ["project", "agent", "os", "task"] {
        let sql = format!(
            "{BASE}
             SELECT {column} AS name, sum(credit_ms * weight)::float8 AS tracked
             FROM base GROUP BY 1 ORDER BY 2 DESC, 1"
        );
        by.push(items(query!(sql).fetch_all(db).await.map_err(failed)?));
    }
    let [by_project, by_agent, by_os, by_task] =
        <[Vec<Item>; 4]>::try_from(by).unwrap_or_else(|_| unreachable!("four columns"));

    // With a language filter the weight already is that language's share.
    let languages_sql = format!(
        "{BASE},
         shares AS (
             SELECT b.credit_ms * b.weight AS credit, l.language,
                    CASE WHEN $6::text IS NULL
                         THEN l.chars::float8 / sum(l.chars) OVER (PARTITION BY b.session_id, b.minute_ms)
                         ELSE 1.0 END AS share
             FROM base b LEFT JOIN minute_languages l
                  ON l.session_id = b.session_id AND l.minute_ms = b.minute_ms
                  AND ($6::text IS NULL OR l.language = $6))
         SELECT coalesce(language, '') AS name, sum(credit * coalesce(share, 1.0))::float8 AS tracked
         FROM shares GROUP BY 1 ORDER BY 2 DESC, 1"
    );
    let by_language = items(query!(languages_sql).fetch_all(db).await.map_err(failed)?);

    let options = sqlx::query(
        "SELECT
             ARRAY(SELECT DISTINCT project FROM agent_sessions WHERE user_id = $1 ORDER BY 1)
                 AS projects,
             ARRAY(SELECT DISTINCT agent FROM agent_sessions WHERE user_id = $1 ORDER BY 1)
                 AS agents,
             ARRAY(SELECT DISTINCT os FROM session_minutes WHERE user_id = $1 ORDER BY 1) AS os,
             ARRAY(SELECT DISTINCT l.language FROM minute_languages l
                   JOIN session_minutes m
                        ON m.session_id = l.session_id AND m.minute_ms = l.minute_ms
                   WHERE m.user_id = $1 ORDER BY 1) AS languages,
             ARRAY(SELECT DISTINCT task FROM session_minutes WHERE user_id = $1 ORDER BY 1)
                 AS tasks",
    )
    .bind(user.id)
    .fetch_one(db)
    .await
    .map_err(failed)?;

    Ok(Json(Stats {
        range,
        timezone: starts.get("tz"),
        tracked_ms: ms(&totals, "tracked"),
        agent_ms: ms(&totals, "agent"),
        sessions: totals.get("sessions"),
        projects: totals.get("projects"),
        unit,
        buckets,
        by_project,
        by_agent,
        by_os,
        by_language,
        by_task,
        options: Options {
            projects: options.get("projects"),
            agents: options.get("agents"),
            os: options.get("os"),
            languages: options.get("languages"),
            tasks: options.get("tasks"),
        },
    }))
}

fn ms(row: &PgRow, column: &str) -> i64 {
    row.get::<f64, _>(column).round() as i64
}

fn items(rows: Vec<PgRow>) -> Vec<Item> {
    rows.iter()
        .map(|row| Item {
            name: row.get("name"),
            tracked_ms: ms(row, "tracked"),
        })
        .collect()
}
