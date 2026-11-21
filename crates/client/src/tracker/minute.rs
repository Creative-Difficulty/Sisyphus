//! One session's minute while it's being recorded.

use std::collections::{BTreeMap, BTreeSet};

use sisyphus_common::heartbeat::{
    Changes, FileChange, Focus, Heartbeat, Prompts, Responses, SLOTS, Slot, VERSION,
};

/// The once-a-second focus readings of one slot.
#[derive(Debug, Clone, Copy, Default)]
pub struct SlotSamples {
    counts: [u8; 8],
    last: Option<Focus>,
}

impl SlotSamples {
    /// No reading yet.
    pub fn is_empty(&self) -> bool {
        self.last.is_none()
    }

    pub fn add(&mut self, focus: Focus) {
        let n = &mut self.counts[index(focus)];
        *n = n.saturating_add(1);
        self.last = Some(focus);
    }

    /// The class seen most often; on a tie the latest of the tied ones.
    /// `Unknown` without readings.
    pub fn focus(&self) -> Focus {
        let max = self.counts.iter().copied().max().unwrap_or(0);
        if max == 0 {
            return Focus::Unknown;
        }
        if let Some(last) = self.last
            && self.counts[index(last)] == max
        {
            return last;
        }
        ALL.into_iter()
            .find(|f| self.counts[index(*f)] == max)
            .unwrap_or(Focus::Unknown)
    }
}

const ALL: [Focus; 8] = [
    Focus::Agent,
    Focus::Terminal,
    Focus::Editor,
    Focus::Browser,
    Focus::Other,
    Focus::Testing,
    Focus::None,
    Focus::Unknown,
];

fn index(focus: Focus) -> usize {
    ALL.iter()
        .position(|f| *f == focus)
        .unwrap_or(ALL.len() - 1)
}

#[derive(Debug, Default)]
pub struct SessionMinute {
    pub slots: [SlotSamples; SLOTS],
    /// Slots in which a prompt was submitted to this session. A prompt
    /// means the user was focused on it (decided 2026-11-21), so those
    /// slots count as `agent` whatever the window readings say. Input still
    /// comes only from the OS.
    pub prompt_slots: [bool; SLOTS],
    pub running: bool,
    /// The project's programs seen in front (relative paths), for
    /// `tested_programs`.
    pub tested_programs: BTreeSet<String>,
    pub prompts: Prompts,
    pub interrupts: u32,
    pub responses: Responses,
    /// Per path: working directory changes, agent claims.
    pub files: BTreeMap<String, (Changes, Changes)>,
}

/// What a heartbeat needs besides the minute itself.
pub struct Meta<'a> {
    pub minute_ms: u64,
    pub agent: &'a str,
    pub agent_session_id: &'a str,
    pub session_name: &'a str,
    pub project: &'a str,
    /// Activity reading was allowed.
    pub activity: bool,
    /// Input per slot, for the whole system.
    pub input: [bool; SLOTS],
}

impl SessionMinute {
    /// The focus a heartbeat carries for slot `i`.
    pub fn focus(&self, i: usize) -> Focus {
        if self.prompt_slots[i] {
            Focus::Agent
        } else {
            self.slots[i].focus()
        }
    }

    /// The first `upto` slots as a heartbeat would carry them, for the
    /// attention formula while the minute is still open.
    pub fn slots_so_far(&self, input: &[bool; SLOTS], upto: usize, activity: bool) -> Vec<Slot> {
        (0..upto.min(SLOTS))
            .map(|i| {
                if activity {
                    Slot {
                        input: input[i],
                        focus: self.focus(i),
                    }
                } else {
                    Slot::UNKNOWN
                }
            })
            .collect()
    }

    /// Sent when the agent ran, its window was in front, or anything
    /// happened in the session.
    pub fn worth_sending(&self) -> bool {
        self.running
            || (0..SLOTS).any(|i| self.focus(i) == Focus::Agent)
            || self.prompts.submitted > 0
            || self.interrupts > 0
            || self.responses.finished > 0
            || !self.files.is_empty()
    }

    pub fn into_heartbeat(self, meta: Meta<'_>) -> Heartbeat {
        let slots = std::array::from_fn(|i| {
            if meta.activity {
                Slot {
                    input: meta.input[i],
                    focus: self.focus(i),
                }
            } else {
                Slot::UNKNOWN
            }
        });
        Heartbeat {
            version: VERSION,
            minute_ms: meta.minute_ms,
            agent: meta.agent.to_string(),
            agent_session_id: meta.agent_session_id.to_string(),
            session_name: meta.session_name.to_string(),
            project: meta.project.to_string(),
            agent_running: self.running,
            activity: meta.activity,
            slots,
            tested_programs: if meta.activity {
                self.tested_programs.into_iter().collect()
            } else {
                Vec::new()
            },
            prompts: self.prompts,
            interrupts: self.interrupts,
            responses: self.responses,
            files: self
                .files
                .into_iter()
                .filter(|(_, (dir, agent))| !dir.is_empty() || !agent.is_empty())
                .map(|(path, (working_dir, agent_edits))| FileChange {
                    path,
                    working_dir,
                    agent_edits,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_focus_is_the_majority() {
        let mut slot = SlotSamples::default();
        assert_eq!(slot.focus(), Focus::Unknown);
        for f in [Focus::Agent, Focus::Agent, Focus::Browser, Focus::Agent] {
            slot.add(f);
        }
        assert_eq!(slot.focus(), Focus::Agent);
        // A short flash into another app doesn't change it; a tie goes to
        // the latest.
        let mut slot = SlotSamples::default();
        for f in [Focus::Editor, Focus::Browser] {
            slot.add(f);
        }
        assert_eq!(slot.focus(), Focus::Browser);
    }

    #[test]
    fn a_prompt_marks_its_slot_as_agent() {
        let mut minute = SessionMinute::default();
        minute.slots[2].add(Focus::Browser);
        minute.prompt_slots[2] = true;
        let hb = minute.into_heartbeat(Meta {
            minute_ms: 60_000,
            agent: "opencode",
            agent_session_id: "s",
            session_name: "t",
            project: "p",
            activity: true,
            input: [false, false, true, false, false, false],
        });
        assert_eq!(
            hb.slots[2],
            Slot {
                input: true,
                focus: Focus::Agent
            }
        );
        assert_eq!(hb.slots[3], Slot::UNKNOWN);
    }

    #[test]
    fn without_activity_slots_are_unknown() {
        let mut minute = SessionMinute {
            running: true,
            ..Default::default()
        };
        minute.slots[0].add(Focus::Agent);
        let hb = minute.into_heartbeat(Meta {
            minute_ms: 60_000,
            agent: "claude-code",
            agent_session_id: "s",
            session_name: "t",
            project: "p",
            activity: false,
            input: [true; SLOTS],
        });
        assert_eq!(hb.slots, [Slot::UNKNOWN; SLOTS]);
        assert!(!hb.activity);
    }
}
