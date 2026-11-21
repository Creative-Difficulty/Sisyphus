//! Types shared by the Sisyphus client and server.
//!
//! How data flows:
//!
//! 1. The client builds one [`Heartbeat`] per agent session per minute, from
//!    the agent's own files and the OS's activity readings.
//! 2. The client signs each heartbeat into a hash chain ([`chain`]) and keeps
//!    it in a local queue, so nothing is lost while offline.
//! 3. The client uploads queued heartbeats to the server in batches ([`api`]).
//!    The server checks that each batch continues the device's chain, records
//!    its own receive time, and stores it. Reviewers read from the server.
//! 4. The server computes attention, credit and struggle per session.

pub mod api;
pub mod attention;
pub mod chain;
pub mod heartbeat;
pub mod hex;

pub use heartbeat::Heartbeat;
