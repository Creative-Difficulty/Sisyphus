//! The offline queue: heartbeats are signed into the device's hash chain as
//! soon as they're built, appended to `queue.jsonl` next to the config, and
//! uploaded in batches. `chain.json` holds the chain tip, so a restart
//! continues the chain.
//!
//! If the server's tip doesn't match (it never received some heartbeats,
//! the device was linked on another server, or local files were lost), the
//! unsent heartbeats are signed again from the server's tip. That's sound:
//! the server never saw them, so nothing it stored changes.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use sisyphus_common::api::{UploadAck, UploadBatch, UploadError};
use sisyphus_common::chain::{ChainTip, ChainWriter, SignedHeartbeat};
use sisyphus_common::heartbeat::{Heartbeat, is_supported};

/// At most this many heartbeats per upload request.
const BATCH: usize = 500;
/// Older heartbeats are dropped beyond this (about a month of one session
/// running all day).
const MAX_QUEUED: usize = 50_000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Queue {
    dir: PathBuf,
    key: SigningKey,
    writer: ChainWriter,
    pending: Vec<SignedHeartbeat>,
    /// The server refused this heartbeat (by seq) as dated in the future
    /// (this computer's clock is ahead): the next batch stops before it.
    /// Cleared once that batch is sent, so it's tried again later.
    hold_from: Option<u64>,
}

/// What an upload attempt did.
#[derive(Debug, PartialEq, Eq)]
pub enum Uploaded {
    /// Nothing to send.
    Nothing,
    /// This many were accepted.
    Sent(usize),
    /// The chain was re-aligned with the server's; try again.
    Realigned,
    /// The server refused heartbeats dated in its future (this computer's
    /// clock is ahead). `before` heartbeats ahead of them can go now; the
    /// `held` rest wait until their minute has passed on the server.
    Early { before: usize, held: usize },
    /// Rejected or unreachable; kept for later.
    Failed(String),
}

impl Queue {
    /// Load the queue in `dir`, continuing its chain.
    pub fn load(dir: PathBuf, key: SigningKey) -> Self {
        let mut pending = read_pending(&dir.join("queue.jsonl"));
        // Heartbeats of a scheme version the server no longer accepts
        // would be refused forever. Dropping them leaves a gap, which the
        // next upload re-aligns.
        pending.retain(|h| is_supported(h.heartbeat.version));
        let saved = std::fs::read_to_string(dir.join("chain.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<ChainTip>(&s).ok())
            .unwrap_or(ChainTip::GENESIS);
        // The queue is written before the tip, so it may be ahead.
        let tip = match pending.last() {
            Some(last) if last.seq >= saved.next_seq => ChainTip {
                next_seq: last.seq + 1,
                prev: last.hash,
            },
            _ => saved,
        };
        Self {
            writer: ChainWriter::resume(key.clone(), tip),
            dir,
            key,
            pending,
            hold_from: None,
        }
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Sign a heartbeat and queue it.
    pub fn push(&mut self, heartbeat: Heartbeat) -> io::Result<()> {
        let signed = self.writer.sign(heartbeat);
        self.pending.push(signed);
        if self.pending.len() > MAX_QUEUED {
            let excess = self.pending.len() - MAX_QUEUED;
            self.pending.drain(..excess);
            return self.save_all();
        }
        let line = serde_json::to_string(self.pending.last().expect("just pushed"))
            .map_err(io::Error::other)?;
        std::fs::create_dir_all(&self.dir)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("queue.jsonl"))?;
        file.write_all(format!("{line}\n").as_bytes())?;
        self.save_tip()
    }

    /// The next batch to upload, if anything is queued.
    pub fn next_batch(&self) -> Option<UploadBatch> {
        let heartbeats: Vec<SignedHeartbeat> = self
            .pending
            .iter()
            .take_while(|h| self.hold_from.is_none_or(|held| h.seq < held))
            .take(BATCH)
            .cloned()
            .collect();
        (!heartbeats.is_empty()).then(|| UploadBatch {
            device: self.writer.device_id(),
            heartbeats,
        })
    }

    /// Apply the server's answer to a batch from [`Queue::next_batch`].
    /// Heartbeats queued in the meantime are kept.
    pub fn apply(&mut self, reply: Result<Reply, String>) -> Uploaded {
        let saved = |queue: &Self, outcome: Uploaded| match queue.save_all() {
            Ok(()) => outcome,
            Err(e) => Uploaded::Failed(format!("could not save the queue: {e}")),
        };
        match reply {
            Ok(Reply::Accepted(ack)) => {
                let before = self.pending.len();
                self.pending.retain(|h| h.seq >= ack.tip.next_seq);
                let sent = before - self.pending.len();
                self.hold_from = None;
                saved(self, Uploaded::Sent(sent))
            }
            Ok(Reply::Rejected(UploadError::OutOfSync { expected })) => {
                match self.realign(expected) {
                    Ok(()) => Uploaded::Realigned,
                    Err(e) => Uploaded::Failed(format!("could not save the queue: {e}")),
                }
            }
            Ok(Reply::Rejected(UploadError::FutureDated { at })) => {
                // The batch started at the front of the queue. When the
                // first is refused there's nothing to send before it: the
                // next round tries the whole queue again.
                self.hold_from = (at > 0)
                    .then(|| self.pending.get(at).map(|h| h.seq))
                    .flatten();
                Uploaded::Early {
                    before: at,
                    held: self.pending.len().saturating_sub(at),
                }
            }
            Ok(Reply::Rejected(other)) => Uploaded::Failed(format!("rejected: {other:?}")),
            Err(e) => Uploaded::Failed(e),
        }
    }

    /// Continue from the server's tip: drop what it already has, or sign
    /// the rest again if our chain doesn't lead to its tip.
    fn realign(&mut self, expected: ChainTip) -> io::Result<()> {
        self.hold_from = None;
        if let Some(at) = self
            .pending
            .iter()
            .position(|h| h.seq == expected.next_seq && h.prev == expected.prev)
        {
            self.pending.drain(..at);
            return self.save_all();
        }
        self.resign(expected)
    }

    fn resign(&mut self, from: ChainTip) -> io::Result<()> {
        self.writer = ChainWriter::resume(self.key.clone(), from);
        self.pending = std::mem::take(&mut self.pending)
            .into_iter()
            .map(|h| self.writer.sign(h.heartbeat))
            .collect();
        self.save_all()
    }

    /// Rewrite the queue file and the tip.
    fn save_all(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut body = String::new();
        for signed in &self.pending {
            body.push_str(&serde_json::to_string(signed).map_err(io::Error::other)?);
            body.push('\n');
        }
        let path = self.dir.join("queue.jsonl");
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, path)?;
        self.save_tip()
    }

    fn save_tip(&self) -> io::Result<()> {
        let path = self.dir.join("chain.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_string(&self.writer.tip()).map_err(io::Error::other)?,
        )?;
        std::fs::rename(tmp, path)
    }
}

/// The server's answer to an upload.
#[derive(Debug)]
pub enum Reply {
    Accepted(UploadAck),
    Rejected(UploadError),
}

/// Send one batch to `server` (without a trailing slash). Blocking.
pub fn send(server: &str, batch: &UploadBatch) -> Result<Reply, String> {
    let http: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = http
        .post(&format!("{server}/api/upload"))
        .send_json(batch)
        .map_err(|e| format!("could not reach the server: {e}"))?;
    match response.status().as_u16() {
        200 => response
            .body_mut()
            .read_json::<UploadAck>()
            .map(Reply::Accepted)
            .map_err(|e| format!("unexpected answer: {e}")),
        400 | 409 => response
            .body_mut()
            .read_json::<UploadError>()
            .map(Reply::Rejected)
            .map_err(|e| format!("unexpected answer: {e}")),
        403 => Err("this device isn't linked to an account".into()),
        status => Err(format!("server answered {status}")),
    }
}

/// The queued heartbeats. A torn last line (a crash mid-write) is skipped.
fn read_pending(path: &std::path::Path) -> Vec<SignedHeartbeat> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sisyphus_common::heartbeat::{Prompts, Responses, SLOTS, Slot, VERSION};

    fn heartbeat(minute: u64) -> Heartbeat {
        Heartbeat {
            version: VERSION,
            minute_ms: minute * 60_000,
            os: sisyphus_common::heartbeat::Os::CURRENT,
            agent: "claude-code".into(),
            agent_session_id: "s".into(),
            session_name: "t".into(),
            project: "p".into(),
            agent_running: true,
            activity: false,
            input_kinds: sisyphus_common::heartbeat::InputKinds::Unknown,
            slots: [Slot::UNKNOWN; SLOTS],
            tested_programs: Vec::new(),
            running_programs: Vec::new(),
            prompts: Prompts::default(),
            interrupts: 0,
            responses: Responses::default(),
            seconds: Default::default(),
            injected: Default::default(),
            client: "0.3.0".into(),
            models: vec![],
            tools: vec![],
            files: Vec::new(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sisyphus-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn queue_survives_a_restart() {
        let dir = temp_dir("queue-restart");
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let mut queue = Queue::load(dir.clone(), key.clone());
        queue.push(heartbeat(1)).unwrap();
        queue.push(heartbeat(2)).unwrap();
        let tip = queue.writer.tip();

        let mut queue = Queue::load(dir.clone(), key);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.writer.tip(), tip);
        queue.push(heartbeat(3)).unwrap();
        let chain = sisyphus_common::chain::verify(
            &queue.pending,
            &queue.writer.device_id(),
            &ChainTip::GENESIS,
        );
        assert_eq!(chain, Ok(queue.writer.tip()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn realigning_drops_what_the_server_has_or_signs_again() {
        let dir = temp_dir("queue-realign");
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let mut queue = Queue::load(dir.clone(), key);
        for m in 0..4 {
            queue.push(heartbeat(m)).unwrap();
        }
        // The server already has the first two.
        let server = ChainTip {
            next_seq: 2,
            prev: queue.pending[1].hash,
        };
        queue.realign(server).unwrap();
        assert_eq!(queue.pending.len(), 2);
        assert_eq!(queue.pending[0].seq, 2);

        // A server that knows another chain: sign again from its tip.
        let other = ChainTip {
            next_seq: 40,
            prev: sisyphus_common::hex::Hex([9u8; 32]),
        };
        queue.realign(other).unwrap();
        let device = queue.writer.device_id();
        assert_eq!(
            sisyphus_common::chain::verify(&queue.pending, &device, &other),
            Ok(queue.writer.tip())
        );
        assert_eq!(queue.pending[0].heartbeat.minute_ms, 2 * 60_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn future_dated_heartbeats_wait() {
        let dir = temp_dir("queue-future");
        let mut queue = Queue::load(dir.clone(), SigningKey::from_bytes(&[3u8; 32]));
        for m in 0..4 {
            queue.push(heartbeat(m)).unwrap();
        }
        let refused = Ok(Reply::Rejected(UploadError::FutureDated { at: 2 }));
        assert_eq!(queue.apply(refused), Uploaded::Early { before: 2, held: 2 });
        // Only the two before the refused one go next.
        let batch = queue.next_batch().unwrap();
        assert_eq!(batch.heartbeats.len(), 2);
        let ack = UploadAck {
            tip: ChainTip {
                next_seq: 2,
                prev: batch.heartbeats[1].hash,
            },
            received_at_ms: 0,
        };
        assert_eq!(queue.apply(Ok(Reply::Accepted(ack))), Uploaded::Sent(2));
        // The rest are tried again later.
        assert_eq!(queue.next_batch().unwrap().heartbeats.len(), 2);
        // Refused from the first: nothing is held back for good.
        let refused = Ok(Reply::Rejected(UploadError::FutureDated { at: 0 }));
        assert_eq!(queue.apply(refused), Uploaded::Early { before: 0, held: 2 });
        assert_eq!(queue.next_batch().unwrap().heartbeats.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
