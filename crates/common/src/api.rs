//! Client <-> server upload protocol.
//!
//! The client uploads queued heartbeats in batches: about every 30 s while
//! online, everything at once after being offline. The server verifies each
//! batch against the chain tip it stored for that device, records its own
//! receive time, and stores the heartbeats. Reviewers only read what the
//! server stored.

use crate::chain::{self, ChainError, ChainTip, DeviceId, SignedHeartbeat};
use crate::heartbeat::VERSION;
use serde::{Deserialize, Serialize};

/// A heartbeat that reaches the server more than this long after the time it
/// claims is treated as recorded offline.
pub const OFFLINE_THRESHOLD_MS: u64 = 5 * 60 * 1000;

/// Consecutive new heartbeats from one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadBatch {
    pub device: DeviceId,
    pub heartbeats: Vec<SignedHeartbeat>,
}

/// Reply to an accepted batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadAck {
    /// The device's chain tip after this batch. The client can drop
    /// everything below `tip.next_seq` from its queue.
    pub tip: ChainTip,
    /// Server clock when the batch was accepted (unix ms).
    pub received_at_ms: u64,
}

/// Reply to a rejected batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadError {
    Empty,
    /// The batch doesn't start where the server's copy of the chain ends.
    /// The client should resend starting from `expected`.
    OutOfSync {
        expected: ChainTip,
    },
    /// The batch was tampered with or signed by the wrong device.
    Invalid(ChainError),
    /// Heartbeat `at` has a scheme version this server doesn't accept.
    UnsupportedVersion {
        at: usize,
    },
    /// Heartbeat `at` is dated in the future: its minute hasn't ended yet
    /// by the server's clock (the device's clock is ahead). The heartbeats
    /// before it may be sent on their own; it and the rest only once their
    /// minute has passed.
    FutureDated {
        at: usize,
    },
}

/// A heartbeat as the server stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredHeartbeat {
    pub signed: SignedHeartbeat,
    /// Server clock when it arrived (unix ms). This is the trusted time.
    pub received_at_ms: u64,
}

impl StoredHeartbeat {
    /// How long after its claimed time the server received it.
    pub fn delay_ms(&self) -> u64 {
        self.received_at_ms
            .saturating_sub(self.signed.heartbeat.minute_ms)
    }

    /// True if it was likely recorded offline. Its claimed time then rests on
    /// the device clock alone, so reviewers see it flagged.
    pub fn is_offline(&self) -> bool {
        self.delay_ms() > OFFLINE_THRESHOLD_MS
    }
}

/// A heartbeat covers one minute from its `minute_ms`.
pub const MINUTE_MS: u64 = 60_000;

/// Whether `heartbeat`'s minute hasn't ended yet at `now_ms` (server
/// clock): it can't have been recorded yet.
pub fn is_future_dated(heartbeat: &crate::heartbeat::Heartbeat, now_ms: u64) -> bool {
    heartbeat.minute_ms.saturating_add(MINUTE_MS) > now_ms
}

/// Server side: check a batch against the device's stored chain tip, at
/// the server's time `now_ms`. Returns the new tip to store.
pub fn verify_batch(
    batch: &UploadBatch,
    tip: &ChainTip,
    now_ms: u64,
) -> Result<ChainTip, UploadError> {
    let first = batch.heartbeats.first().ok_or(UploadError::Empty)?;
    if let Some(at) = batch
        .heartbeats
        .iter()
        .position(|h| h.heartbeat.version != VERSION)
    {
        return Err(UploadError::UnsupportedVersion { at });
    }
    if first.seq != tip.next_seq {
        return Err(UploadError::OutOfSync { expected: *tip });
    }
    let new_tip =
        chain::verify(&batch.heartbeats, &batch.device, tip).map_err(UploadError::Invalid)?;
    if let Some(at) = batch
        .heartbeats
        .iter()
        .position(|h| is_future_dated(&h.heartbeat, now_ms))
    {
        return Err(UploadError::FutureDated { at });
    }
    Ok(new_tip)
}

/// Domain tag for the signature that links a device to a user account.
/// Bump the version when the signed message changes.
pub const DEVICE_LINK_CONTEXT: &[u8] = b"sisyphus/device-link/v1\0";

/// What the app signs with its device key to prove it owns that key when
/// exchanging the one-time login `code` (see the server's
/// `POST /auth/app/token`).
pub fn device_link_message(code: &str) -> Vec<u8> {
    let mut message = DEVICE_LINK_CONTEXT.to_vec();
    message.extend_from_slice(code.as_bytes());
    message
}

/// Body of `POST /auth/app/token`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTokenRequest {
    /// One-time code the server handed to the app's loopback listener.
    pub code: String,
    /// The secret whose SHA-256 (hex) the app sent as `challenge` when it
    /// started the login.
    pub verifier: String,
    /// The device's ed25519 public key.
    pub device: DeviceId,
    /// Device signature over [`device_link_message`] of `code`.
    pub signature: crate::hex::SigBytes,
}

/// Reply to a successful `POST /auth/app/token`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTokenResponse {
    /// The account's display name, if Hack Club Auth provided one.
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Signed app requests
//
// The app has no session cookie or token: it proves who it is by signing
// each request with its device key. The server checks the signature, that
// the time is close to its own clock, that the same request wasn't sent
// before, and that the device is linked to an account. Heartbeat uploads
// don't need this: every heartbeat in them is already signed and chained.

/// Domain tag for signed app requests. Bump the version when the signed
/// message changes.
pub const REQUEST_CONTEXT: &[u8] = b"sisyphus/request/v1\0";

/// The device's public key, hex.
pub const HEADER_DEVICE: &str = "sisyphus-device";
/// When the request was signed (unix ms, device clock).
pub const HEADER_TIME: &str = "sisyphus-time";
/// Device signature over [`request_message`], hex.
pub const HEADER_SIGNATURE: &str = "sisyphus-signature";

/// Requests signed further than this from the server's clock are refused.
pub const REQUEST_MAX_SKEW_MS: u64 = 5 * 60 * 1000;

/// What the app signs for a request: the method, the path with its query,
/// the time and the SHA-256 of the body.
pub fn request_message(method: &str, path_and_query: &str, time_ms: u64, body: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut message = REQUEST_CONTEXT.to_vec();
    message.extend_from_slice(
        format!(
            "{method}\n{path_and_query}\n{time_ms}\n{}",
            hex::encode(Sha256::digest(body))
        )
        .as_bytes(),
    );
    message
}

/// Reply to `GET /api/app/today?start_ms=…`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Today {
    /// Credited time since `start_ms` (the start of the user's local day).
    pub tracked_ms: u64,
    /// Time an agent ran since then; a minute with several agents running
    /// counts once.
    #[serde(default)]
    pub agent_ms: u64,
}
