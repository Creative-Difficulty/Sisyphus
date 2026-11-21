//! Turns heartbeats into credited time (decided 2026-11-21).
//!
//! Per session and minute:
//! - With activity allowed, attention is followed slot by slot (10 s):
//!   - input while the session's agent window is in front sets it to 1.0
//!   - while that window stays in front (from the second slot in a row) it
//!     halves every 2 minutes; otherwise (another app, the first slot back
//!     in the window without input, or an unknown slot) every 20 seconds
//!   - it carries over to the session's next minute, and starts at 0.0
//!     after a gap
//!   - the minute's multiplier is the average over its six slots.
//! - Without activity (or when no slot's focus is known), only agent events
//!   count: a prompt gives 1.0, an interrupt 0.5, and attention falls back
//!   to 0.0 at the next minute.
//! - Only minutes in which the agent ran earn credit; time after a turn
//!   ends earns nothing for now. Attention is still followed through those
//!   minutes.
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
use sqlx::{PgConnection, PgPool, Row};

/// The version of the calculation below.
pub const FORMULA: i32 = 1;

const MINUTE_MS: i64 = 60_000;
const SLOT_SECS: f64 = 10.0;
/// Attention's half-life while the agent's window stays in front, and
/// otherwise.
const STAY_HALF_LIFE_SECS: f64 = 120.0;
const LEAVE_HALF_LIFE_SECS: f64 = 20.0;

/// What's left of attention after one slot with half-life `secs`.
fn decay(secs: f64) -> f64 {
    0.5f64.powf(SLOT_SECS / secs)
}
/// Without activity: what agent events are worth.
const PROMPT: f64 = 1.0;
const INTERRUPT: f64 = 0.5;

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

/// Where the session's previous minute left off.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Carry {
    pub attention: f64,
    pub last_slot_agent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// 0.0 to 1.0, before sharing with concurrent sessions.
    pub multiplier: f64,
    pub carry: Carry,
}

/// One minute of one session. `carry` is `None` when the session's
/// previous minute wasn't recorded.
pub fn compute(body: &Body, carry: Option<Carry>) -> Outcome {
    let known = body.slots.iter().any(|s| s.focus != Focus::Unknown);
    let (value, carry) = if body.activity && known {
        let mut carry = carry.unwrap_or_default();
        let mut sum = 0.0;
        for slot in &body.slots {
            let agent = slot.focus == Focus::Agent;
            carry.attention = if agent && slot.input {
                1.0
            } else if agent && carry.last_slot_agent {
                carry.attention * decay(STAY_HALF_LIFE_SECS)
            } else {
                carry.attention * decay(LEAVE_HALF_LIFE_SECS)
            };
            carry.last_slot_agent = agent;
            sum += carry.attention;
        }
        let value = if body.slots.is_empty() {
            0.0
        } else {
            sum / body.slots.len() as f64
        };
        (value, carry)
    } else {
        let mut value: f64 = 0.0;
        if body.prompts.submitted > 0 {
            value = value.max(PROMPT);
        }
        if body.interrupts > 0 {
            value = value.max(INTERRUPT);
        }
        (value, Carry::default())
    };
    Outcome {
        multiplier: if body.agent_running {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        },
        carry,
    }
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
        "SELECT attention_end, last_slot_agent FROM session_minutes
         WHERE session_id = $1 AND minute_ms = $2",
    )
    .bind(&id)
    .bind(body.minute_ms - MINUTE_MS)
    .fetch_optional(&mut *db)
    .await?
    .map(|row| Carry {
        attention: row.get("attention_end"),
        last_slot_agent: row.get("last_slot_agent"),
    });
    let outcome = compute(&body, carry);
    let inserted = sqlx::query(
        "INSERT INTO session_minutes
             (session_id, minute_ms, user_id, device_id, seq, attention_end,
              last_slot_agent, multiplier, credit_ms, ai_ms, offline, formula)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, $9, $10, $11)
         ON CONFLICT (session_id, minute_ms) DO NOTHING",
    )
    .bind(&id)
    .bind(body.minute_ms)
    .bind(hb.user_id)
    .bind(hb.device_id)
    .bind(hb.seq)
    .bind(outcome.carry.attention)
    .bind(outcome.carry.last_slot_agent)
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

    fn body(activity: bool, running: bool, slots: &[(bool, Focus)]) -> Body {
        Body {
            minute_ms: 0,
            agent: "claude-code".into(),
            agent_session_id: "s".into(),
            session_name: String::new(),
            project: String::new(),
            agent_running: running,
            activity,
            slots: slots
                .iter()
                .map(|&(input, focus)| Slot { input, focus })
                .collect(),
            prompts: Prompts::default(),
            interrupts: 0,
        }
    }

    const A: Focus = Focus::Agent;
    const B: Focus = Focus::Browser;

    #[test]
    fn input_in_the_agent_window_is_full_attention() {
        let out = compute(&body(true, true, &[(true, A); 6]), None);
        assert_eq!(out.multiplier, 1.0);
        assert_eq!(
            out.carry,
            Carry {
                attention: 1.0,
                last_slot_agent: true
            }
        );
    }

    #[test]
    fn staying_decays_slowly_and_leaving_quickly() {
        let mut slots = vec![(true, A)];
        slots.extend([(false, A); 5]);
        let stay = compute(&body(true, true, &slots), None).carry.attention;
        // Five slots at a 2-minute half-life.
        assert!((stay - 0.5f64.powf(50.0 / 120.0)).abs() < 1e-9);

        let mut slots = vec![(true, A)];
        slots.extend([(false, B); 5]);
        let leave = compute(&body(true, true, &slots), None).carry.attention;
        assert!((leave - 0.5f64.powf(50.0 / 20.0)).abs() < 1e-9);
    }

    #[test]
    fn switching_back_without_input_doesnt_slow_the_decay() {
        let carry = Carry {
            attention: 1.0,
            last_slot_agent: false,
        };
        let out = compute(&body(true, true, &[(false, A), (false, A)]), Some(carry));
        // First slot back counts as leaving, the second as staying.
        assert!(
            (out.carry.attention - decay(LEAVE_HALF_LIFE_SECS) * decay(STAY_HALF_LIFE_SECS)).abs()
                < 1e-9
        );
    }

    #[test]
    fn input_elsewhere_doesnt_reset_attention() {
        let out = compute(&body(true, true, &[(true, B); 6]), None);
        assert_eq!(out.multiplier, 0.0);
    }

    #[test]
    fn no_credit_without_the_agent_running() {
        let out = compute(&body(true, false, &[(true, A); 6]), None);
        assert_eq!(out.multiplier, 0.0);
        // Attention still carries into the next minute.
        assert_eq!(out.carry.attention, 1.0);
    }

    #[test]
    fn without_activity_only_prompts_and_interrupts_count() {
        let mut b = body(false, true, &[(false, Focus::Unknown); 6]);
        assert_eq!(compute(&b, None).multiplier, 0.0);
        b.interrupts = 1;
        assert_eq!(compute(&b, None).multiplier, 0.5);
        b.prompts.submitted = 1;
        let out = compute(
            &b,
            Some(Carry {
                attention: 1.0,
                last_slot_agent: true,
            }),
        );
        assert_eq!(out.multiplier, 1.0);
        // Back to zero at the next minute.
        assert_eq!(out.carry, Carry::default());
    }

    #[test]
    fn session_ids_are_uuid_shaped_and_stable() {
        let a = session_id("dev", "claude-code", "x");
        assert_eq!(a.len(), 36);
        assert_eq!(a, session_id("dev", "claude-code", "x"));
        assert_ne!(a, session_id("dev", "opencode", "x"));
    }
}
