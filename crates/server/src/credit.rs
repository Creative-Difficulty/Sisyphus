//! Turns heartbeats into credited time (decided 2026-11-21).
//!
//! The attention formula itself lives in `sisyphus_common::attention`, so
//! the client shows exactly what the server computes. Per session and
//! minute it gives a multiplier from 0.0 to 1.0 (only minutes in which the
//! agent ran earn credit). Then:
//! - Concurrent sessions of a user share each real minute: if their
//!   multipliers add up to more than 1.0, each gets its share.
//! - Offline heartbeats count; they're flagged on the heartbeat.
//!
//! Not decided yet, so not applied: handling mouse jigglers, the struggle
//! value, and edit-claim checks.
//!
//! The results are derived data in `session_minutes`, tagged with
//! [`FORMULA`]. When the formula changes, bump it: rows of older formulas
//! are rebuilt from the stored heartbeats on the next start.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use sisyphus_common::attention;
use sisyphus_common::heartbeat;
use sqlx::{PgConnection, PgPool, Row};

/// The version of the calculation below.
/// 1: per slot, leaving half-life 20 s. 2: per slot, 30 s. 3: one
/// attention value per heartbeat.
pub const FORMULA: i32 = 3;

const MINUTE_MS: i64 = 60_000;
/// The parts of a stored heartbeat body the calculation reads. Lenient, so
/// older bodies still parse.
#[derive(Debug, Deserialize)]
pub struct Body {
    pub minute_ms: i64,
    pub agent: String,
    pub agent_session_id: String,
    #[serde(default)]
    pub session_name: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub agent_running: bool,
    #[serde(default)]
    pub activity: bool,
    #[serde(default)]
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub prompts: Prompts,
    #[serde(default)]
    pub interrupts: u32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Slot {
    #[serde(default)]
    pub input: bool,
    #[serde(default)]
    pub focus: Focus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Focus {
    Agent,
    Terminal,
    Editor,
    Browser,
    Other,
    Testing,
    None,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Default, Deserialize)]
pub struct Prompts {
    #[serde(default)]
    pub submitted: u32,
}

pub use sisyphus_common::attention::{Carry, Outcome};

/// One minute of one session, with the shared formula.
pub fn compute(body: &Body, carry: Option<Carry>) -> Outcome {
    let slots: Vec<heartbeat::Slot> = body
        .slots
        .iter()
        .map(|s| heartbeat::Slot {
            input: s.input,
            focus: match s.focus {
                Focus::Agent => heartbeat::Focus::Agent,
                Focus::Terminal => heartbeat::Focus::Terminal,
                Focus::Editor => heartbeat::Focus::Editor,
                Focus::Browser => heartbeat::Focus::Browser,
                Focus::Other => heartbeat::Focus::Other,
                Focus::Testing => heartbeat::Focus::Testing,
                Focus::None => heartbeat::Focus::None,
                Focus::Unknown => heartbeat::Focus::Unknown,
            },
        })
        .collect();
    attention::minute(
        &attention::Minute {
            activity: body.activity,
            agent_running: body.agent_running,
            slots: &slots,
            prompts: body.prompts.submitted,
            interrupts: body.interrupts,
        },
        carry,
    )
}

/// The server's id for a session: a UUID-formatted SHA-256 of the device,
/// the agent and the agent's session id, so sessions of different devices
/// or agents never collide.
pub fn session_id(device_id: &str, agent: &str, agent_session_id: &str) -> String {
    let digest = Sha256::digest(format!("{device_id}\0{agent}\0{agent_session_id}"));
    let h = hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// A stored heartbeat to credit.
pub struct Stored<'a> {
    pub user_id: i64,
    pub device_id: &'a str,
    pub seq: i64,
    pub offline: bool,
    pub body: &'a str,
}

/// Credit one heartbeat: update its session and add its minute. Returns
/// the minute, which then has to be shared with [`share_minute`]; `None`
/// if the body can't be read or the minute was already credited.
pub async fn process(db: &mut PgConnection, hb: Stored<'_>) -> sqlx::Result<Option<i64>> {
    let Ok(body) = serde_json::from_str::<Body>(hb.body) else {
        return Ok(None);
    };
    let id = session_id(hb.device_id, &body.agent, &body.agent_session_id);
    sqlx::query(
        "INSERT INTO agent_sessions
             (id, user_id, device_id, agent, agent_session_id, name, project,
              first_minute_ms, last_minute_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
         ON CONFLICT (id) DO UPDATE SET
             name = CASE WHEN EXCLUDED.last_minute_ms >= agent_sessions.last_minute_ms
                         THEN EXCLUDED.name ELSE agent_sessions.name END,
             project = EXCLUDED.project,
             first_minute_ms = LEAST(agent_sessions.first_minute_ms, EXCLUDED.first_minute_ms),
             last_minute_ms = GREATEST(agent_sessions.last_minute_ms, EXCLUDED.last_minute_ms)",
    )
    .bind(&id)
    .bind(hb.user_id)
    .bind(hb.device_id)
    .bind(&body.agent)
    .bind(&body.agent_session_id)
    .bind(&body.session_name)
    .bind(&body.project)
    .bind(body.minute_ms)
    .execute(&mut *db)
    .await?;

    let carry = sqlx::query(
        "SELECT attention_end FROM session_minutes
         WHERE session_id = $1 AND minute_ms = $2",
    )
    .bind(&id)
    .bind(body.minute_ms - MINUTE_MS)
    .fetch_optional(&mut *db)
    .await?
    .map(|row| Carry {
        attention: row.get("attention_end"),
    });
    let outcome = compute(&body, carry);
    let inserted = sqlx::query(
        "INSERT INTO session_minutes
             (session_id, minute_ms, user_id, device_id, seq, attention_end,
              multiplier, credit_ms, ai_ms, offline, formula)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 0, $8, $9, $10)
         ON CONFLICT (session_id, minute_ms) DO NOTHING",
    )
    .bind(&id)
    .bind(body.minute_ms)
    .bind(hb.user_id)
    .bind(hb.device_id)
    .bind(hb.seq)
    .bind(outcome.carry.attention)
    .bind(outcome.multiplier)
    .bind(if body.agent_running { MINUTE_MS } else { 0 })
    .bind(hb.offline)
    .bind(FORMULA)
    .execute(&mut *db)
    .await?
    .rows_affected();
    Ok((inserted > 0).then_some(body.minute_ms))
}

/// Share one real minute among the user's sessions: if their multipliers
/// add up to more than 1.0, each gets its share of the minute.
pub async fn share_minute(db: &mut PgConnection, user_id: i64, minute_ms: i64) -> sqlx::Result<()> {
    sqlx::query(
        "WITH total AS (
             SELECT sum(multiplier) AS sum FROM session_minutes
             WHERE user_id = $1 AND minute_ms = $2
         )
         UPDATE session_minutes SET credit_ms = round(
             $3 * multiplier / GREATEST(1.0, (SELECT sum FROM total)))::bigint
         WHERE user_id = $1 AND minute_ms = $2",
    )
    .bind(user_id)
    .bind(minute_ms)
    .bind(MINUTE_MS as f64)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Credit every stored heartbeat of a linked device that isn't credited
/// yet, after dropping rows of older formulas. Runs on start.
pub async fn backfill(db: &PgPool) -> sqlx::Result<usize> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM session_minutes WHERE formula <> $1")
        .bind(FORMULA)
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query(
        "SELECT h.device_id, h.seq, h.offline, h.body::text AS body, d.user_id
         FROM heartbeats h JOIN devices d ON d.device_id = h.device_id
         WHERE d.user_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM session_minutes m
                           WHERE m.device_id = h.device_id AND m.seq = h.seq)
         ORDER BY h.device_id, h.seq",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut minutes = std::collections::BTreeSet::new();
    for row in &rows {
        let user_id: i64 = row.get("user_id");
        let stored = Stored {
            user_id,
            device_id: row.get("device_id"),
            seq: row.get("seq"),
            offline: row.get("offline"),
            body: row.get("body"),
        };
        if let Some(minute) = process(&mut tx, stored).await? {
            minutes.insert((user_id, minute));
        }
    }
    for (user_id, minute) in &minutes {
        share_minute(&mut tx, *user_id, *minute).await?;
    }
    tx.commit().await?;
    Ok(minutes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_bodies_use_the_shared_formula() {
        let body: Body = serde_json::from_str(
            r#"{"minute_ms": 0, "agent": "claude-code", "agent_session_id": "s",
                "agent_running": true, "activity": true,
                "slots": [{"input": true, "focus": "agent"}, {"input": false, "focus": "future_class"}]}"#,
        )
        .unwrap();
        // Input in the agent's window: the whole heartbeat is 1.0.
        assert_eq!(compute(&body, None).multiplier, 1.0);
        // Without it, unknown classes count like another app, so the agent's
        // window wasn't in front for most of the minute: leaving.
        let idle: Body = serde_json::from_str(
            r#"{"minute_ms": 0, "agent": "claude-code", "agent_session_id": "s",
                "agent_running": true, "activity": true,
                "slots": [{"input": false, "focus": "agent"},
                          {"input": false, "focus": "future_class"},
                          {"input": false, "focus": "future_class"}]}"#,
        )
        .unwrap();
        let out = compute(&idle, Some(Carry { attention: 1.0 }));
        let leave = attention::decay(attention::LEAVE_HALF_LIFE_SECS);
        assert!((out.multiplier - leave).abs() < 1e-9);
    }

    #[test]
    fn session_ids_are_uuid_shaped_and_stable() {
        let a = session_id("dev", "claude-code", "x");
        assert_eq!(a.len(), 36);
        assert_eq!(a, session_id("dev", "claude-code", "x"));
        assert_ne!(a, session_id("dev", "opencode", "x"));
    }
}
