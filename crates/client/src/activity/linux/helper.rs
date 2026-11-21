//! Wayland: key and mouse counts from the input helper service
//! ([`crate::input_helper`]), which the user installs once with their
//! password. It sends lines `<keys> <mouse>` over a Unix socket that only
//! this user may read; which keys were pressed never leaves it.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::activity::InputCounts;
use crate::input_helper::SOCKET;

/// Reconnect this often while the helper isn't there.
const RETRY: Duration = Duration::from_secs(5);

/// Whether the helper is installed and running.
pub fn available() -> bool {
    UnixStream::connect(SOCKET).is_ok()
}

#[derive(Default)]
struct Shared {
    counts: InputCounts,
    connected: bool,
}

/// A connection to the helper, kept up on its own thread. Stops when
/// dropped.
pub struct Helper {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

impl Helper {
    pub fn start() -> Option<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_shared, thread_stop) = (shared.clone(), stop.clone());
        std::thread::Builder::new()
            .name("input-helper".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    if let Ok(stream) = UnixStream::connect(SOCKET) {
                        read(stream, &thread_shared, &thread_stop);
                    }
                    lock(&thread_shared).connected = false;
                    let mut waited = Duration::ZERO;
                    while waited < RETRY && !thread_stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(250));
                        waited += Duration::from_millis(250);
                    }
                }
            })
            .ok()?;
        Some(Self { shared, stop })
    }

    /// What was counted since the last call; `None` while the helper isn't
    /// connected.
    pub fn take(&self) -> Option<InputCounts> {
        let mut shared = lock(&self.shared);
        let counts = std::mem::take(&mut shared.counts);
        shared.connected.then_some(counts)
    }

    pub fn connected(&self) -> bool {
        lock(&self.shared).connected
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn lock(m: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read count lines until the helper goes away or we're stopped.
fn read(stream: UnixStream, shared: &Mutex<Shared>, stop: &AtomicBool) {
    // Wake up regularly to notice `stop`; the helper only writes when
    // something happened.
    if stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .is_err()
    {
        return;
    }
    lock(shared).connected = true;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    while !stop.load(Ordering::Relaxed) {
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                if let Some(counts) = parse(&line) {
                    let mut shared = lock(shared);
                    shared.counts.keys = shared.counts.keys.saturating_add(counts.keys);
                    shared.counts.mouse = shared.counts.mouse.saturating_add(counts.mouse);
                }
                line.clear();
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
    }
}

/// `"<keys> <mouse>\n"`.
fn parse(line: &str) -> Option<InputCounts> {
    let mut parts = line.split_whitespace();
    let keys = parts.next()?.parse().ok()?;
    let mouse = parts.next()?.parse().ok()?;
    Some(InputCounts { keys, mouse })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_count_lines() {
        assert_eq!(
            parse("3 120\n"),
            Some(InputCounts {
                keys: 3,
                mouse: 120
            })
        );
        assert_eq!(parse("garbage\n"), None);
    }
}
