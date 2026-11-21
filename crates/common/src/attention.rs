//! The attention formula (decided 2026-11-21; leaving half-life 30 s since
//! formula 2). The server credits time with
//! it; the client runs the same code only to show it and compare.
//!
//! Per session and minute:
//! - With activity allowed, attention is followed slot by slot (10 s):
//!   input while the session's agent window is in front sets it to 1.0;
//!   while that window stays in front (from the second slot in a row) it
//!   halves every 2 minutes, otherwise (another app, the first slot back
//!   without input, an unknown slot) every 30 seconds. It carries over to
//!   the session's next minute and starts at 0.0 after a gap. The minute's
//!   multiplier is the average over its slots.
//! - Without activity (or when no slot's focus is known) only agent events
//!   count: a prompt gives 1.0, an interrupt 0.5, and attention is back to
//!   0.0 at the next minute.
//! - Only minutes in which the agent ran earn credit.
//!
//! Floats are fine here: none of this is hashed or signed.

use crate::heartbeat::{Focus, Slot};

pub const SLOT_SECS: f64 = 10.0;
/// Attention's half-life while the agent's window stays in front.
pub const STAY_HALF_LIFE_SECS: f64 = 120.0;
/// Attention's half-life otherwise.
pub const LEAVE_HALF_LIFE_SECS: f64 = 30.0;
/// Without activity: what agent events are worth.
pub const PROMPT: f64 = 1.0;
pub const INTERRUPT: f64 = 0.5;

/// What's left of attention after one slot with a half-life of `secs`.
pub fn decay(secs: f64) -> f64 {
    0.5f64.powf(SLOT_SECS / secs)
}

/// Where a session's previous minute left off.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Carry {
    pub attention: f64,
    pub last_slot_agent: bool,
}

/// One minute (or the part of it seen so far) of one session.
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
    /// 0.0 to 1.0, before sharing with concurrent sessions; 0.0 when the
    /// agent didn't run.
    pub multiplier: f64,
    /// The average attention over the slots, whether or not the agent ran.
    pub attention: f64,
    pub carry: Carry,
}

/// `carry` is `None` when the session's previous minute wasn't recorded.
pub fn minute(m: &Minute<'_>, carry: Option<Carry>) -> Outcome {
    let known = m.slots.iter().any(|s| s.focus != Focus::Unknown);
    let (attention, carry) = if m.activity && known {
        let mut carry = carry.unwrap_or_default();
        let mut sum = 0.0;
        for slot in m.slots {
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
        let average = if m.slots.is_empty() {
            0.0
        } else {
            sum / m.slots.len() as f64
        };
        (average, carry)
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
        multiplier: if m.agent_running { attention } else { 0.0 },
        attention,
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

    fn run(activity: bool, running: bool, list: &[(bool, Focus)], carry: Option<Carry>) -> Outcome {
        let slots = slots(list);
        minute(
            &Minute { activity, agent_running: running, slots: &slots, prompts: 0, interrupts: 0 },
            carry,
        )
    }

    #[test]
    fn input_in_the_agent_window_is_full_attention() {
        let out = run(true, true, &[(true, A); 6], None);
        assert_eq!(out.multiplier, 1.0);
        assert_eq!(out.carry, Carry { attention: 1.0, last_slot_agent: true });
    }

    #[test]
    fn staying_decays_slowly_and_leaving_quickly() {
        let mut list = vec![(true, A)];
        list.extend([(false, A); 5]);
        let stay = run(true, true, &list, None).carry.attention;
        assert!((stay - 0.5f64.powf(50.0 / 120.0)).abs() < 1e-9);

        let mut list = vec![(true, A)];
        list.extend([(false, B); 5]);
        let leave = run(true, true, &list, None).carry.attention;
        assert!((leave - 0.5f64.powf(50.0 / 30.0)).abs() < 1e-9);
    }

    #[test]
    fn switching_back_without_input_doesnt_slow_the_decay() {
        let carry = Carry { attention: 1.0, last_slot_agent: false };
        let out = run(true, true, &[(false, A), (false, A)], Some(carry));
        let expected = decay(LEAVE_HALF_LIFE_SECS) * decay(STAY_HALF_LIFE_SECS);
        assert!((out.carry.attention - expected).abs() < 1e-9);
    }

    #[test]
    fn input_elsewhere_doesnt_reset_attention() {
        assert_eq!(run(true, true, &[(true, B); 6], None).multiplier, 0.0);
    }

    #[test]
    fn no_credit_without_the_agent_running() {
        let out = run(true, false, &[(true, A); 6], None);
        assert_eq!(out.multiplier, 0.0);
        assert_eq!(out.attention, 1.0);
        assert_eq!(out.carry.attention, 1.0);
    }

    #[test]
    fn without_activity_only_prompts_and_interrupts_count() {
        let unknown = slots(&[(false, Focus::Unknown); 6]);
        let mut m = Minute { activity: false, agent_running: true, slots: &unknown, prompts: 0, interrupts: 0 };
        assert_eq!(minute(&m, None).multiplier, 0.0);
        m.interrupts = 1;
        assert_eq!(minute(&m, None).multiplier, 0.5);
        m.prompts = 1;
        let out = minute(&m, Some(Carry { attention: 1.0, last_slot_agent: true }));
        assert_eq!(out.multiplier, 1.0);
        assert_eq!(out.carry, Carry::default());
    }
}
