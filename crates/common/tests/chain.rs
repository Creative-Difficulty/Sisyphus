use ed25519_dalek::SigningKey;
use sisyphus_common::chain::{ChainError, ChainTip, ChainWriter, SignedHeartbeat, verify};
use sisyphus_common::heartbeat::{Category, Heartbeat, Source};

fn writer() -> ChainWriter {
    ChainWriter::new(SigningKey::from_bytes(&[7u8; 32]))
}

fn coding(i: u64) -> Heartbeat {
    let mut hb = Heartbeat::new(1_000 + i * 60_000, Source::Editor, Category::Coding, "nvim");
    hb.project = Some("demo".into());
    hb.language = Some("rust".into());
    hb.edits.typed_chars = 100 + i as u32;
    hb
}

fn signed(n: u64) -> (ChainWriter, Vec<SignedHeartbeat>) {
    let mut w = writer();
    let hbs = (0..n).map(|i| w.sign(coding(i))).collect();
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
    hbs[4].heartbeat.edits.typed_chars = 99_999;
    assert_eq!(
        verify(&hbs, &w.device_id(), &ChainTip::GENESIS),
        Err(ChainError::BadHash { at: 4 })
    );
}

#[test]
fn changed_source_is_detected() {
    let (w, mut hbs) = signed(5);
    hbs[2].heartbeat.source = Source::OsDaemon;
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
    hbs[1].heartbeat.edits.typed_chars = 5_000;
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
    hbs.push(w2.sign(coding(3)));
    assert_eq!(verify(&hbs, &w.device_id(), &ChainTip::GENESIS), Ok(w2.tip()));
}

#[test]
fn heartbeat_json_roundtrip() {
    let (_, hbs) = signed(1);
    let json = serde_json::to_string(&hbs[0]).unwrap();
    assert_eq!(serde_json::from_str::<SignedHeartbeat>(&json).unwrap(), hbs[0]);
}
