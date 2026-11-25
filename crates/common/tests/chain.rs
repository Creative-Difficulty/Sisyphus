use ed25519_dalek::SigningKey;
use sisyphus_common::chain::{ChainError, ChainTip, ChainWriter, SignedHeartbeat, verify};
use sisyphus_common::heartbeat::{
    Changes, FileChange, Focus, Heartbeat, InputKinds, Os, Prompts, Responses, Slot, VERSION,
};

fn writer() -> ChainWriter {
    ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]))
}

fn minute(i: u64) -> Heartbeat {
    Heartbeat {
        version: VERSION,
        minute_ms: 1_795_201_380_000 + i * 60_000,
        os: Os::Linux,
        agent: "claude-code".into(),
        agent_session_id: "77ae4435-adc7-40d4-934f-f70f97ced5e4".into(),
        session_name: "Demo".into(),
        project: "demo".into(),
        agent_running: true,
        activity: None,
        input_kinds: InputKinds::Exact,
        slots: [Slot {
            input: true,
            keys: 3,
            mouse: 1,
            focus: Focus::Agent,
        }; 6],
        tested_programs: None,
        running_programs: None,
        prompts: Prompts {
            typed_chars: 100 + i as u32,
            at: vec![5],
            ..Prompts::default()
        },
        interrupts: 0,
        responses: Responses::default(),
        seconds: Default::default(),
        injected: Default::default(),
        client: "0.3.0".into(),
        models: vec![],
        tools: vec![],
        programs: vec![],
        tool_calls: vec![],
        files: vec![FileChange {
            path: "src/main.rs".into(),
            working_dir: Changes {
                added: 20,
                removed: 3,
            },
            agent_edits: None,
        }],
    }
}

/// `hb` in the shape a v9 client wrote it: every field it had is present.
#[allow(dead_code)]
fn as_v9(mut hb: Heartbeat) -> Heartbeat {
    hb.version = 9;
    hb.activity = Some(true);
    hb.tested_programs = Some(vec![]);
    hb.running_programs = Some(vec![]);
    hb.prompts.submitted = Some(hb.prompts.at.len() as u32);
    hb.prompts.corrections = Some(0);
    hb.prompts.at = vec![];
    hb.responses.finished = Some(0);
    hb.responses.chars = Some(0);
    hb.client = String::new();
    hb.seconds = Some(Default::default());
    for f in &mut hb.files {
        f.agent_edits = Some(f.working_dir);
    }
    hb
}

fn signed(n: u64) -> (ChainWriter, Vec<SignedHeartbeat>) {
    let mut w = writer();
    let hbs = (0..n).map(|i| w.sign(minute(i))).collect();
    (w, hbs)
}

#[test]
fn valid_chain_verifies() {
    let (w, hbs) = signed(10);
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Ok(w.tip())
    );
}

#[test]
fn edited_heartbeat_is_detected() {
    let (w, mut hbs) = signed(10);
    hbs[4].heartbeat.prompts.typed_chars = 99_999;
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Err(ChainError::BadHash { at: 4 })
    );
}

#[test]
fn changed_slot_is_detected() {
    let (w, mut hbs) = signed(5);
    hbs[2].heartbeat.slots[3].focus = Focus::Browser;
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Err(ChainError::BadHash { at: 2 })
    );
}

#[test]
fn dropped_heartbeat_is_detected() {
    let (w, mut hbs) = signed(10);
    hbs.remove(5);
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Err(ChainError::WrongSeq { at: 5 })
    );
}

#[test]
fn reordered_heartbeats_are_detected() {
    let (w, mut hbs) = signed(10);
    hbs.swap(3, 4);
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Err(ChainError::WrongSeq { at: 3 })
    );
}

#[test]
fn rehashed_forgery_fails_signature() {
    let (w, mut hbs) = signed(3);
    hbs[1].heartbeat.prompts.typed_chars = 5_000;
    hbs[1].hash = hbs[1].expected_hash();
    hbs[2].prev = hbs[1].hash;
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Err(ChainError::BadSignature { at: 1 })
    );
}

#[test]
fn other_device_cannot_claim_heartbeats() {
    let (_, hbs) = signed(3);
    let other = ChainWriter::new(SigningKey::from_bytes(&[9u8; 32]));
    assert_eq!(
        verify(&hbs, &other.device_id(), &ChainTip::GENESIS),
        Err(ChainError::BadSignature { at: 0 })
    );
}

#[test]
fn resumed_writer_continues_chain() {
    let (w, mut hbs) = signed(3);
    let mut w2 = ChainWriter::resume(SigningKey::from_bytes(&[7u8; 32]), w.tip());
    hbs.push(w2.sign(minute(3)));
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Ok(w2.tip())
    );
}

#[test]
fn heartbeat_json_roundtrip() {
    let (_, hbs) = signed(1);
    let json = serde_json::to_string(&hbs[0]).unwrap();
    assert_eq!(
        serde_json::from_str::<SignedHeartbeat>(&json).unwrap(),
        hbs[0]
    );
}

/// v9 heartbeats from clients that haven't updated must hash exactly as
/// they did before v10 added fields.
#[test]
fn v9_heartbeats_hash_as_before() {
    let mut hb = minute(0);
    hb.agent_session_id = "s".into();
    hb.prompts.typed_chars = 100;
    hb.files = vec![];
    let hb = as_v9(hb);
    let signed = writer().sign(hb);
    assert_eq!(
        serde_json::to_string(&signed.hash).unwrap(),
        "\"43a399a15c8ed5cc3195aba03f969ba56c11c5767a7fda7be102eb439d136e17\""
    );
    // Its stored JSON reads back the same.
    let json = serde_json::to_string(&signed).unwrap();
    assert!(!json.contains("\"client\"") && !json.contains("\"tool_calls\""));
    let back: SignedHeartbeat = serde_json::from_str(&json).unwrap();
    assert_eq!(back.expected_hash(), signed.hash);
}

#[test]
fn the_version_is_part_of_the_hash() {
    let a = writer().sign(minute(0));
    let b = writer().sign(as_v9(minute(0)));
    assert_ne!(a.hash, b.hash);
}

/// Nothing that can be computed is written any more, and the helpers
/// compute it for old and new heartbeats alike.
#[test]
fn v12_leaves_out_what_can_be_computed() {
    let hb = minute(0);
    let json = serde_json::to_string(&hb).unwrap();
    for field in [
        "submitted",
        "corrections",
        "finished",
        "\"chars\"",
        "activity",
        "tested_programs",
        "running_programs",
        "agent_edits",
    ] {
        assert!(!json.contains(field), "{field} in {json}");
    }
    let old = as_v9(minute(0));
    assert_eq!(hb.prompts.count(), 1);
    assert_eq!(old.prompts.count(), 1);
    assert!(hb.activity() && old.activity());
    assert_eq!(hb.responses.count(), 0);
}
