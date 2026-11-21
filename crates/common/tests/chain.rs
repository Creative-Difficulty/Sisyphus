use ed25519_dalek::SigningKey;
use sisyphus_common::chain::{ChainError, ChainTip, ChainWriter, SignedHeartbeat, verify};
use sisyphus_common::heartbeat::{Changes, FileChange, Focus, Heartbeat, InputKinds, Prompts, Responses, Slot, VERSION};

fn writer() -> ChainWriter {
    ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]))
}

fn minute(i: u64) -> Heartbeat {
    Heartbeat {
        version: VERSION,
        minute_ms: 1_795_201_380_000 + i * 60_000,
        agent: "claude-code".into(),
        agent_session_id: "77ae4435-adc7-40d4-934f-f70f97ced5e4".into(),
        session_name: "Demo".into(),
        project: "demo".into(),
        agent_running: true,
        activity: true,
        input_kinds: InputKinds::Exact,
        slots: [Slot { input: true, keys: 3, mouse: 1, focus: Focus::Agent }; 6],
        tested_programs: vec![],
        prompts: Prompts { submitted: 1, typed_chars: 100 + i as u32, ..Prompts::default() },
        interrupts: 0,
        responses: Responses::default(),
        files: vec![FileChange {
            path: "src/main.rs".into(),
            working_dir: Changes { added: 20, removed: 3 },
            agent_edits: Changes { added: 20, removed: 3 },
        }],
    }
}

fn signed(n: u64) -> (ChainWriter, Vec<SignedHeartbeat>) {
    let mut w = writer();
    let hbs = (0..n).map(|i| w.sign(minute(i))).collect();
    (w, hbs)
}

#[test]
fn valid_chain_verifies() {
    let (w, hbs) = signed(10);
    assert_eq!(verify(&hbs, &w.device_id(), &ChainTip::GENESIS), Ok(w.tip()));
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
    assert_eq!(verify(&hbs, &w.device_id(), &ChainTip::GENESIS), Ok(w2.tip()));
}

#[test]
fn heartbeat_json_roundtrip() {
    let (_, hbs) = signed(1);
    let json = serde_json::to_string(&hbs[0]).unwrap();
    assert_eq!(serde_json::from_str::<SignedHeartbeat>(&json).unwrap(), hbs[0]);
}
