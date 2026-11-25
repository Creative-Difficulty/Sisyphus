use ed25519_dalek::SigningKey;
use sisyphus_common::api::{StoredHeartbeat, UploadBatch, UploadError, verify_batch as verify_at};
use sisyphus_common::chain::{ChainError, ChainTip, ChainWriter, SignedHeartbeat};
use sisyphus_common::heartbeat::{
    Changes, FileChange, Focus, Heartbeat, InputKinds, Os, Prompts, Responses, Slot, VERSION,
};

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
        activity: true,
        input_kinds: InputKinds::Exact,
        slots: [Slot {
            input: true,
            keys: 3,
            mouse: 1,
            focus: Focus::Agent,
        }; 6],
        tested_programs: vec![],
        running_programs: vec![],
        prompts: Prompts {
            submitted: 1,
            typed_chars: 100 + i as u32,
            ..Prompts::default()
        },
        interrupts: 0,
        responses: Responses::default(),
        seconds: Default::default(),
        injected: Default::default(),
        client: "0.3.0".into(),
        models: vec![],
        tools: vec![],
        files: vec![FileChange {
            path: "src/main.rs".into(),
            working_dir: Changes {
                added: 20,
                removed: 3,
            },
            agent_edits: Changes {
                added: 20,
                removed: 3,
            },
        }],
    }
}

fn signed(n: u64) -> (ChainWriter, Vec<SignedHeartbeat>) {
    let mut w = ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]));
    let hbs = (0..n).map(|i| w.sign(minute(i))).collect();
    (w, hbs)
}

/// Long after every test minute.
const NOW: u64 = 1_900_000_000_000;

fn verify_batch(batch: &UploadBatch, tip: &ChainTip) -> Result<ChainTip, UploadError> {
    verify_at(batch, tip, NOW)
}

fn batch(w: &ChainWriter, hbs: &[SignedHeartbeat]) -> UploadBatch {
    UploadBatch {
        device: w.device_id(),
        heartbeats: hbs.to_vec(),
    }
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
        Err(UploadError::OutOfSync {
            expected: ChainTip::GENESIS
        })
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
    assert_eq!(
        verify_batch(&batch(&w, &[]), &ChainTip::GENESIS),
        Err(UploadError::Empty)
    );
}

#[test]
fn late_heartbeats_are_flagged_offline() {
    let (_, hbs) = signed(1);
    let t = hbs[0].heartbeat.minute_ms;
    let live = StoredHeartbeat {
        signed: hbs[0].clone(),
        received_at_ms: t + 30_000,
    };
    let late = StoredHeartbeat {
        signed: hbs[0].clone(),
        received_at_ms: t + 3 * 3_600_000,
    };
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

/// Clients that haven't updated yet still send v9; they are accepted, also
/// mixed with v10 in one batch (a queue from before an update).
#[test]
fn v9_is_still_accepted() {
    let mut w = ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]));
    let mut old = minute(0);
    old.version = 9;
    old.client = String::new();
    let hbs = vec![w.sign(old), w.sign(minute(1))];
    assert!(verify_batch(&batch(&w, &hbs), &ChainTip::GENESIS).is_ok());
}

#[test]
fn future_dated_heartbeats_are_rejected() {
    let (w, hbs) = signed(4);
    // Minute 2 ends exactly now: fine. Minute 3 hasn't ended yet.
    let now = hbs[2].heartbeat.minute_ms + 60_000;
    assert_eq!(
        verify_at(&batch(&w, &hbs), &ChainTip::GENESIS, now),
        Err(UploadError::FutureDated { at: 3 })
    );
    assert!(verify_at(&batch(&w, &hbs[..3]), &ChainTip::GENESIS, now).is_ok());
}

#[test]
fn minutes_must_start_on_a_whole_minute() {
    // One real minute claimed at T, T+1 s, …: each would be a "different"
    // minute and earn a full minute of credit.
    let mut w = ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]));
    let hbs: Vec<_> = (0..3)
        .map(|i| {
            let mut hb = minute(0);
            hb.minute_ms += i * 1_000;
            w.sign(hb)
        })
        .collect();
    assert_eq!(
        verify_batch(&batch(&w, &hbs), &ChainTip::GENESIS),
        Err(UploadError::Misaligned { at: 1 })
    );
}

#[test]
fn heartbeats_an_hour_late_are_stale() {
    let (_, hbs) = signed(1);
    let t = hbs[0].heartbeat.minute_ms;
    let at = |delay| StoredHeartbeat {
        signed: hbs[0].clone(),
        received_at_ms: t + delay,
    };
    assert!(!at(10 * 60_000).is_stale());
    assert!(at(10 * 60_000).is_offline());
    assert!(!at(60 * 60_000).is_stale());
    assert!(at(60 * 60_000 + 1).is_stale());
}
