use ed25519_dalek::SigningKey;
use sisyphus_common::api::{StoredHeartbeat, UploadBatch, UploadError, verify_batch};
use sisyphus_common::chain::{ChainError, ChainTip, ChainWriter, SignedHeartbeat};
use sisyphus_common::heartbeat::{Changes, FileChange, Focus, Heartbeat, Prompts, Responses, Slot, VERSION};

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
        slots: [Slot { input: true, focus: Focus::Agent }; 6],
        prompts: Prompts { submitted: 1, typed_chars: 100 + i as u32, ..Prompts::default() },
        responses: Responses::default(),
        files: vec![FileChange {
            path: "src/main.rs".into(),
            working_dir: Changes { added: 20, removed: 3 },
            agent_edits: Changes { added: 20, removed: 3 },
        }],
    }
}

fn signed(n: u64) -> (ChainWriter, Vec<SignedHeartbeat>) {
    let mut w = ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]));
    let hbs = (0..n)
        .map(|i| w.sign(minute(i)))
        .collect();
    (w, hbs)
}

fn batch(w: &ChainWriter, hbs: &[SignedHeartbeat]) -> UploadBatch {
    UploadBatch { device: w.device_id(), heartbeats: hbs.to_vec() }
}

#[test]
fn batches_continue_each_other() {
    let (w, hbs) = signed(10);
    let tip = verify_batch(&batch(&w, &hbs[..4]), &ChainTip::GENESIS).unwrap();
    assert_eq!(tip.next_seq, 4);
    let tip = verify_batch(&batch(&w, &hbs[4..]), &tip).unwrap();
    assert_eq!(tip, w.tip());
}

#[test]
fn skipping_ahead_is_out_of_sync() {
    let (w, hbs) = signed(10);
    assert_eq!(
        verify_batch(&batch(&w, &hbs[3..]), &ChainTip::GENESIS),
        Err(UploadError::OutOfSync { expected: ChainTip::GENESIS })
    );
}

#[test]
fn resending_is_out_of_sync() {
    let (w, hbs) = signed(10);
    let tip = verify_batch(&batch(&w, &hbs[..5]), &ChainTip::GENESIS).unwrap();
    assert_eq!(
        verify_batch(&batch(&w, &hbs[..5]), &tip),
        Err(UploadError::OutOfSync { expected: tip })
    );
}

#[test]
fn tampered_batch_is_rejected() {
    let (w, mut hbs) = signed(5);
    hbs[2].heartbeat.prompts.typed_chars = 50_000;
    assert_eq!(
        verify_batch(&batch(&w, &hbs), &ChainTip::GENESIS),
        Err(UploadError::Invalid(ChainError::BadHash { at: 2 }))
    );
}

#[test]
fn empty_batch_is_rejected() {
    let (w, _) = signed(0);
    assert_eq!(verify_batch(&batch(&w, &[]), &ChainTip::GENESIS), Err(UploadError::Empty));
}

#[test]
fn late_heartbeats_are_flagged_offline() {
    let (_, hbs) = signed(1);
    let t = hbs[0].heartbeat.minute_ms;
    let live = StoredHeartbeat { signed: hbs[0].clone(), received_at_ms: t + 30_000 };
    let late = StoredHeartbeat { signed: hbs[0].clone(), received_at_ms: t + 3 * 3_600_000 };
    assert!(!live.is_offline());
    assert!(late.is_offline());
}

#[test]
fn other_versions_are_rejected() {
    let mut w = ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]));
    let mut old = minute(1);
    old.version = 1;
    let hbs = vec![w.sign(minute(0)), w.sign(old)];
    assert_eq!(
        verify_batch(&batch(&w, &hbs), &ChainTip::GENESIS),
        Err(UploadError::UnsupportedVersion { at: 1 })
    );
}
