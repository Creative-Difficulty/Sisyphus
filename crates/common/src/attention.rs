//! The attention formula (decided 2026-11-21). The server credits time with
//! it; the client runs the same code only to show it and compare.
//!
//! Attention is one value per heartbeat (one session's minute):
//! - With activity allowed:
//!   - any input in a slot where the session's agent window is in front
//!     makes the whole heartbeat 1.0 (writing a prompt is such input, and
//!     the slot a prompt is sent in counts as the agent's)
//!   - otherwise it decays from the session's previous heartbeat: with a
//!     2-minute half-life if the agent's window was in front for most of
//!     the minute (staying), with a 30-second half-life if not (leaving)
//!   - after a gap (no heartbeat for the previous minute) it starts at 0.0
//! - Without activity (or when no slot's focus is known) only agent events
//!   count: a prompt gives 1.0, an interrupt 0.5, otherwise 0.0, and
//!   nothing carries over.
//! - Only minutes in which the agent ran earn credit: the multiplier is the
//!   attention then, 0.0 otherwise.
//!
//! The heartbeat's 10-second slots are the readings this is computed from.
//!
//! Floats are fine here: none of this is hashed or signed.

use crate::heartbeat::{Focus, Slot};

/// Seconds in one heartbeat.
pub const HEARTBEAT_SECS: f64 = 60.0;
/// Attention's half-life while the agent's window stays in front.
pub const STAY_HALF_LIFE_SECS: f64 = 120.0;
/// Attention's half-life otherwise.
pub const LEAVE_HALF_LIFE_SECS: f64 = 30.0;
/// Without activity: what agent events are worth.
pub const PROMPT: f64 = 1.0;
pub const INTERRUPT: f64 = 0.5;

/// What's left of attention after one heartbeat with a half-life of `secs`.
pub fn decay(secs: f64) -> f64 {
    0.5f64.powf(HEARTBEAT_SECS / secs)
}

/// Where a session's previous heartbeat left off.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Carry {
    pub attention: f64,
}

/// One heartbeat (or the part of its minute seen so far) of one session.
#[derive(Debug, Clone, Copy)]
pub struct Minute<'a> {
    pub activity: bool,
    pub agent_running: bool,
    pub slots: &'a [Slot],
    pub prompts: u32,
    pub interrupts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// The heartbeat's attention, 0.0 to 1.0.
    pub attention: f64,
    /// 0.0 to 1.0, before sharing with concurrent sessions: the attention
    /// when the agent ran, 0.0 otherwise.
    pub multiplier: f64,
    /// What the session's next heartbeat starts from.
    pub carry: Carry,
}

/// `carry` is `None` when the session's previous heartbeat wasn't recorded.
pub fn minute(m: &Minute<'_>, carry: Option<Carry>) -> Outcome {
    let known = m.slots.iter().any(|s| s.focus != Focus::Unknown);
    let (attention, carry) = if m.activity && known {
        let engaged = m.slots.iter().any(|s| s.focus == Focus::Agent && s.input);
        let attention = if engaged {
            1.0
        } else {
            let in_front = m.slots.iter().filter(|s| s.focus == Focus::Agent).count();
            let half_life = if in_front * 2 > m.slots.len() {
                STAY_HALF_LIFE_SECS
            } else {
                LEAVE_HALF_LIFE_SECS
            };
            carry.unwrap_or_default().attention * decay(half_life)
        };
        (attention, Carry { attention })
    } else {
        let mut value: f64 = 0.0;
        if m.prompts > 0 {
            value = value.max(PROMPT);
        }
        if m.interrupts > 0 {
            value = value.max(INTERRUPT);
        }
        (value, Carry::default())
    };
    let attention = attention.clamp(0.0, 1.0);
    Outcome {
        attention,
        multiplier: if m.agent_running { attention } else { 0.0 },
        carry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Focus = Focus::Agent;
    const B: Focus = Focus::Browser;

    fn slots(list: &[(bool, Focus)]) -> Vec<Slot> {
        list.iter().map(|&(input, focus)| Slot { input, focus }).collect()
    }

    fn run(list: &[(bool, Focus)], carry: Option<Carry>) -> Outcome {
        let slots = slots(list);
        minute(
            &Minute { activity: true, agent_running: true, slots: &slots, prompts: 0, interrupts: 0 },
            carry,
        )
    }

    fn full() -> Option<Carry> {
        Some(Carry { attention: 1.0 })
    }

    #[test]
    fn any_input_in_the_agent_window_makes_the_whole_heartbeat_full() {
        // One slot of typing (say, a prompt), the rest of the minute elsewhere.
        let out = run(&[(false, B), (true, A), (false, B), (false, B), (false, B), (false, B)], None);
        assert_eq!(out.attention, 1.0);
        assert_eq!(out.multiplier, 1.0);
        assert_eq!(out.carry.attention, 1.0);
    }

    #[test]
    fn staying_decays_with_a_two_minute_half_life() {
        let out = run(&[(false, A); 6], full());
        assert!((out.attention - 0.5f64.powf(0.5)).abs() < 1e-9);
        // Most of the minute in the window still counts as staying.
        let mostly = run(&[(false, A), (false, A), (false, A), (false, A), (false, B), (false, B)], full());
        assert_eq!(mostly.attention, out.attention);
    }

    #[test]
    fn leaving_decays_with_a_thirty_second_half_life() {
        let out = run(&[(false, B); 6], full());
        assert!((out.attention - 0.25).abs() < 1e-9);
        // Half the minute isn't most of it.
        let half = run(&[(false, A), (false, A), (false, A), (false, B), (false, B), (false, B)], full());
        assert_eq!(half.attention, out.attention);
    }

    #[test]
    fn input_elsewhere_doesnt_count() {
        assert_eq!(run(&[(true, B); 6], None).attention, 0.0);
    }

    #[test]
    fn a_gap_starts_at_zero() {
        assert_eq!(run(&[(false, A); 6], None).attention, 0.0);
    }

    #[test]
    fn no_credit_without_the_agent_running() {
        let slots = slots(&[(true, A); 6]);
        let out = minute(
            &Minute { activity: true, agent_running: false, slots: &slots, prompts: 0, interrupts: 0 },
            None,
        );
        assert_eq!(out.attention, 1.0);
        assert_eq!(out.multiplier, 0.0);
        assert_eq!(out.carry.attention, 1.0);
    }

    #[test]
    fn without_activity_only_prompts_and_interrupts_count() {
        let unknown = slots(&[(false, Focus::Unknown); 6]);
        let mut m = Minute { activity: false, agent_running: true, slots: &unknown, prompts: 0, interrupts: 0 };
        assert_eq!(minute(&m, full()).multiplier, 0.0);
        m.interrupts = 1;
        assert_eq!(minute(&m, None).multiplier, 0.5);
        m.prompts = 1;
        let out = minute(&m, None);
        assert_eq!(out.multiplier, 1.0);
        assert_eq!(out.carry, Carry::default());
    }
}
