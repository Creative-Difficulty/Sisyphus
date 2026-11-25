//! Signs heartbeats into a hash chain.
//!
//! Heartbeats can sit in the client's offline queue for hours before upload.
//! The chain lets the server detect if any were edited, dropped, reordered or
//! duplicated in the meantime: each [`SignedHeartbeat`] has a sequence number,
//! the hash of the previous one, and an ed25519 signature from the device key.
//!
//! This proves heartbeats were not changed after they were signed. It does not
//! prove they are true: the key lives on the user's machine, so a modified
//! client can sign anything. Truthfulness is checked separately on the server.

use crate::heartbeat::Heartbeat;
use crate::hex::{Hash, Hex, SigBytes};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A device is identified by its ed25519 public key.
pub type DeviceId = Hex<32>;

/// The end of a chain: what the next heartbeat must continue from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainTip {
    /// Sequence number the next heartbeat must have.
    pub next_seq: u64,
    /// Hash the next heartbeat must link to.
    pub prev: Hash,
}

impl ChainTip {
    /// The tip of a chain with nothing in it yet.
    pub const GENESIS: Self = Self {
        next_seq: 0,
        prev: Hash::ZERO,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHeartbeat {
    pub seq: u64,
    /// Hash of the previous signed heartbeat ([`Hash::ZERO`] for the first).
    pub prev: Hash,
    pub heartbeat: Heartbeat,
    /// Hash of `seq`, `prev` and `heartbeat`.
    pub hash: Hash,
    /// Device signature over `hash`.
    pub sig: SigBytes,
}

impl SignedHeartbeat {
    /// Recompute the hash from the contents.
    pub fn expected_hash(&self) -> Hash {
        hash_of(self.seq, &self.prev, &self.heartbeat)
    }
}

fn hash_of(seq: u64, prev: &Hash, heartbeat: &Heartbeat) -> Hash {
    #[derive(Serialize)]
    struct Body<'a> {
        seq: u64,
        prev: &'a Hash,
        heartbeat: &'a Heartbeat,
    }
    // Struct fields serialize in declaration order and heartbeats contain no
    // floats or maps, so this encoding is deterministic.
    let body = serde_json::to_vec(&Body {
        seq,
        prev,
        heartbeat,
    })
    .expect("heartbeat serialization cannot fail");
    let mut d = Sha256::new();
    // Domain tag, by the heartbeat's scheme version (accepted versions are
    // checked before hashing); bump the version when the format changes.
    d.update(format!("sisyphus/heartbeat/v{}\0", heartbeat.version).as_bytes());
    d.update(&body);
    Hex(d.finalize().into())
}

/// Signs heartbeats on the client. Holds only the device key and the chain
/// tip; persist the tip and use [`ChainWriter::resume`] after a restart.
pub struct ChainWriter {
    key: SigningKey,
    tip: ChainTip,
}

impl ChainWriter {
    pub fn new(key: SigningKey) -> Self {
        Self::resume(key, ChainTip::GENESIS)
    }

    pub fn resume(key: SigningKey, tip: ChainTip) -> Self {
        Self { key, tip }
    }

    pub fn tip(&self) -> ChainTip {
        self.tip
    }

    pub fn device_id(&self) -> DeviceId {
        Hex(self.key.verifying_key().to_bytes())
    }

    pub fn sign(&mut self, heartbeat: Heartbeat) -> SignedHeartbeat {
        let seq = self.tip.next_seq;
        let prev = self.tip.prev;
        let hash = hash_of(seq, &prev, &heartbeat);
        let sig = Hex(self.key.sign(&hash.0).to_bytes());
        self.tip = ChainTip {
            next_seq: seq + 1,
            prev: hash,
        };
        SignedHeartbeat {
            seq,
            prev,
            heartbeat,
            hash,
            sig,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainError {
    /// The device id is not a valid ed25519 public key.
    BadDeviceId,
    /// Heartbeat `at` has the wrong sequence number (gap, duplicate or reorder).
    WrongSeq { at: usize },
    /// Heartbeat `at` does not link to the previous one.
    BrokenLink { at: usize },
    /// Heartbeat `at` was changed after it was signed.
    BadHash { at: usize },
    /// Heartbeat `at` was not signed by this device.
    BadSignature { at: usize },
}

/// Check that `heartbeats` continue the chain from `tip` and were signed by
/// `device`. Returns the new tip.
pub fn verify(
    heartbeats: &[SignedHeartbeat],
    device: &DeviceId,
    tip: &ChainTip,
) -> Result<ChainTip, ChainError> {
    let key = VerifyingKey::from_bytes(&device.0).map_err(|_| ChainError::BadDeviceId)?;
    let mut tip = *tip;
    for (at, hb) in heartbeats.iter().enumerate() {
        if hb.seq != tip.next_seq {
            return Err(ChainError::WrongSeq { at });
        }
        if hb.prev != tip.prev {
            return Err(ChainError::BrokenLink { at });
        }
        if hb.expected_hash() != hb.hash {
            return Err(ChainError::BadHash { at });
        }
        key.verify(&hb.hash.0, &Signature::from_bytes(&hb.sig.0))
            .map_err(|_| ChainError::BadSignature { at })?;
        tip = ChainTip {
            next_seq: hb.seq + 1,
            prev: hb.hash,
        };
    }
    Ok(tip)
}
