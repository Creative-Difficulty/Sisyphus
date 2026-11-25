//! One session's minute while it's being recorded.

use std::collections::{BTreeMap, BTreeSet};

use sisyphus_common::heartbeat::{
    Changes, FileChange, Focus, Heartbeat, Injected, InputKinds, Os, Prompts, Responses, SLOTS,
    Slot, ToolCalls, ToolKind, VERSION,
};

/// One slot's input, for the whole system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlotInput {
    /// Any input, from the idle time.
    pub any: bool,
    pub keys: u32,
    pub mouse: u32,
}

/// A minute's input, for the whole system.
#[derive(Debug, Clone, Copy, Default)]
pub struct MinuteInput {
    pub slots: [SlotInput; SLOTS],
    /// Input that came from software (see `Heartbeat::injected`).
    pub injected: Injected,
    /// How keys and mouse were told apart: the weakest way seen in the
    /// minute (`None` before the first reading).
    kinds: Option<InputKinds>,
}

impl MinuteInput {
    /// Add counted input at `second` (0 to 59) of the minute.
    pub fn add_counts(&mut self, second: usize, keys: u32, mouse: u32, injected: Injected) {
        let slot = &mut self.slots[(second / 10).min(SLOTS - 1)];
        slot.keys = slot.keys.saturating_add(keys);
        slot.mouse = slot.mouse.saturating_add(mouse);
        self.injected.keys = self.injected.keys.saturating_add(injected.keys);
        self.injected.mouse = self.injected.mouse.saturating_add(injected.mouse);
    }

    pub fn add_kinds(&mut self, kinds: InputKinds) {
        self.kinds = Some(self.kinds.map_or(kinds, |k| k.min(kinds)));
    }

    pub fn kinds(&self) -> InputKinds {
        self.kinds.unwrap_or(InputKinds::Unknown)
    }

    /// Slot `i` as a heartbeat carries it, with `focus`.
    fn slot(&self, i: usize, focus: Focus) -> Slot {
        let s = self.slots[i];
        let told_apart = self.kinds() != InputKinds::Unknown;
        Slot {
            input: s.any || s.keys > 0 || s.mouse > 0,
            keys: if told_apart { s.keys } else { 0 },
            mouse: if told_apart { s.mouse } else { 0 },
            focus,
        }
    }
}

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
    /// Slots in which a prompt was submitted to this session (not ones a
    /// program sent). A prompt means the user was focused on it (decided
    /// 2026-11-21), so those slots count as `agent` when the window in
    /// front could be the agent's: a terminal or editor the process match
    /// missed, or an unknown reading. Not when a browser, another app, a
    /// test program or nothing was in front (2026-11-23): a prompt sent by
    /// a script while the user types elsewhere mustn't make that typing
    /// count as typing to the agent. Input still comes only from the OS.
    pub prompt_slots: [bool; SLOTS],
    pub running: bool,
    /// The project's programs seen in front (relative paths), for
    /// `tested_programs`.
    pub tested_programs: BTreeSet<String>,
    /// The project's programs seen running (relative paths), for
    /// `running_programs`.
    pub running_programs: BTreeSet<String>,
    pub prompts: Prompts,
    pub interrupts: u32,
    pub responses: Responses,
    /// Per path: working directory changes, agent claims.
    pub files: BTreeMap<String, (Changes, Changes)>,
    /// The models that worked in the session this minute.
    pub models: BTreeSet<String>,
    /// Per kind: tool calls started, and milliseconds of the calls that
    /// finished this minute.
    pub tools: BTreeMap<ToolKind, (u32, u32)>,
}

/// The client's version, written into every heartbeat.
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a heartbeat needs besides the minute itself.
pub struct Meta<'a> {
    pub minute_ms: u64,
    pub agent: &'a str,
    pub agent_session_id: &'a str,
    pub session_name: &'a str,
    pub project: &'a str,
    /// Activity was read.
    pub activity: bool,
    /// Input per slot, for the whole system.
    pub input: MinuteInput,
}

impl SessionMinute {
    /// The focus a heartbeat carries for slot `i`.
    pub fn focus(&self, i: usize) -> Focus {
        let read = self.slots[i].focus();
        match read {
            Focus::Terminal | Focus::Editor | Focus::Unknown if self.prompt_slots[i] => {
                Focus::Agent
            }
            _ => read,
        }
    }

    /// The first `upto` slots as a heartbeat would carry them, for the
    /// attention formula while the minute is still open.
    pub fn slots_so_far(&self, input: &MinuteInput, upto: usize, activity: bool) -> Vec<Slot> {
        (0..upto.min(SLOTS))
            .map(|i| {
                if activity {
                    input.slot(i, self.focus(i))
                } else {
                    Slot::UNKNOWN
                }
            })
            .collect()
    }

    /// Sent when the agent ran, its window was in front, anything happened
    /// in the session, or the project's program ran.
    pub fn worth_sending(&self) -> bool {
        self.running
            || (0..SLOTS).any(|i| self.focus(i) == Focus::Agent)
            || self.prompts.submitted > 0
            || self.interrupts > 0
            || self.responses.finished > 0
            || !self.files.is_empty()
            || !self.running_programs.is_empty()
    }

    pub fn into_heartbeat(self, meta: Meta<'_>) -> Heartbeat {
        let slots = std::array::from_fn(|i| {
            if meta.activity {
                meta.input.slot(i, self.focus(i))
            } else {
                Slot::UNKNOWN
            }
        });
        Heartbeat {
            version: VERSION,
            minute_ms: meta.minute_ms,
            os: Os::CURRENT,
            agent: meta.agent.to_string(),
            agent_session_id: meta.agent_session_id.to_string(),
            session_name: meta.session_name.to_string(),
            project: meta.project.to_string(),
            agent_running: self.running,
            activity: meta.activity,
            input_kinds: if meta.activity {
                meta.input.kinds()
            } else {
                InputKinds::Unknown
            },
            slots,
            tested_programs: if meta.activity {
                self.tested_programs.into_iter().collect()
            } else {
                Vec::new()
            },
            running_programs: self.running_programs.into_iter().collect(),
            prompts: self.prompts,
            interrupts: self.interrupts,
            responses: self.responses,
            seconds: None,
            injected: if meta.activity {
                meta.input.injected
            } else {
                Injected::default()
            },
            client: CLIENT_VERSION.to_string(),
            models: self.models.into_iter().collect(),
            tools: self
                .tools
                .into_iter()
                .map(|(kind, (calls, ms))| ToolCalls { kind, calls, ms })
                .collect(),
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
    fn a_prompt_marks_its_slot_as_agent_only_from_a_possible_agent_window() {
        for (read, expected) in [
            (None, Focus::Agent),
            (Some(Focus::Terminal), Focus::Agent),
            (Some(Focus::Editor), Focus::Agent),
            (Some(Focus::Browser), Focus::Browser),
            (Some(Focus::Other), Focus::Other),
            (Some(Focus::Testing), Focus::Testing),
            (Some(Focus::None), Focus::None),
        ] {
            let mut minute = SessionMinute::default();
            if let Some(read) = read {
                minute.slots[1].add(read);
            }
            minute.prompt_slots[1] = true;
            assert_eq!(minute.focus(1), expected, "{read:?}");
        }
    }

    #[test]
    fn a_prompt_marks_its_slot_as_agent() {
        let mut minute = SessionMinute::default();
        minute.slots[2].add(Focus::Terminal);
        minute.prompt_slots[2] = true;
        let hb = minute.into_heartbeat(Meta {
            minute_ms: 60_000,
            agent: "opencode",
            agent_session_id: "s",
            session_name: "t",
            project: "p",
            activity: true,
            input: {
                let mut input = MinuteInput::default();
                input.slots[2].any = true;
                input.add_counts(25, 12, 3, Injected { keys: 2, mouse: 0 });
                input.add_kinds(InputKinds::Exact);
                input
            },
        });
        assert_eq!(hb.seconds, None);
        assert_eq!(hb.injected, Injected { keys: 2, mouse: 0 });
        assert_eq!(
            hb.slots[2],
            Slot {
                input: true,
                keys: 12,
                mouse: 3,
                focus: Focus::Agent
            }
        );
        assert_eq!(hb.input_kinds, InputKinds::Exact);
        assert_eq!(hb.slots[3], Slot::UNKNOWN);
    }

    #[test]
    fn a_running_program_alone_sends_a_heartbeat() {
        let mut minute = SessionMinute::default();
        minute.slots[0].add(Focus::Testing);
        assert!(!minute.worth_sending());
        minute.running_programs.insert("target/debug/game".into());
        assert!(minute.worth_sending());
        let hb = minute.into_heartbeat(Meta {
            minute_ms: 60_000,
            agent: "claude-code",
            agent_session_id: "s",
            session_name: "t",
            project: "p",
            activity: true,
            input: MinuteInput::default(),
        });
        assert_eq!(hb.running_programs, ["target/debug/game"]);
        assert!(!hb.agent_running);
    }

    #[test]
    fn tools_models_and_client() {
        let mut minute = SessionMinute {
            running: true,
            ..Default::default()
        };
        minute.models.insert("claude-opus-5-5".into());
        minute.models.insert("claude-haiku-5-5".into());
        minute.tools.insert(ToolKind::Read, (3, 120));
        minute.tools.insert(ToolKind::Shell, (1, 0));
        let hb = minute.into_heartbeat(Meta {
            minute_ms: 60_000,
            agent: "claude-code",
            agent_session_id: "s",
            session_name: "t",
            project: "p",
            activity: true,
            input: MinuteInput::default(),
        });
        assert_eq!(hb.client, env!("CARGO_PKG_VERSION"));
        assert_eq!(hb.models, ["claude-haiku-5-5", "claude-opus-5-5"]);
        assert_eq!(
            hb.tools,
            [
                ToolCalls {
                    kind: ToolKind::Shell,
                    calls: 1,
                    ms: 0
                },
                ToolCalls {
                    kind: ToolKind::Read,
                    calls: 3,
                    ms: 120
                },
            ]
        );
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
            input: MinuteInput::default(),
        });
        assert_eq!(hb.slots, [Slot::UNKNOWN; SLOTS]);
        assert!(!hb.activity);
    }
}
