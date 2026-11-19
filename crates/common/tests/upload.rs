use ed25519_dalek::SigningKey;
use sisyphus_common::api::{StoredHeartbeat, UploadBatch, UploadError, verify_batch};
use sisyphus_common::chain::{ChainError, ChainTip, ChainWriter, SignedHeartbeat};
use sisyphus_common::heartbeat::{Category, Heartbeat, Source};

fn signed(n: u64) -> (ChainWriter, Vec<SignedHeartbeat>) {
    let mut w = ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]));
    let hbs = (0..n)
        .map(|i| w.sign(Heartbeat::new(1_000 + i * 60_000, Source::Editor, Category::Coding, "vscode")))
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
    hbs[2].heartbeat.edits.typed_chars = 50_000;
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
    let t = hbs[0].heartbeat.time_ms;
    let live = StoredHeartbeat { signed: hbs[0].clone(), received_at_ms: t + 30_000 };
    let late = StoredHeartbeat { signed: hbs[0].clone(), received_at_ms: t + 3 * 3_600_000 };
    assert!(!live.is_offline());
    assert!(late.is_offline());
}
