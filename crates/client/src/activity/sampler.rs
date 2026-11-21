//! Telling key presses from mouse events where the OS only gives the time
//! of the last input (Windows, X11), without a keyboard hook.
//!
//! A thread reads the last input time, the cursor position and whether a
//! mouse button is down 25 times a second. When the last input time moves
//! on, the input was a mouse event if the cursor moved or a button is down,
//! otherwise a key press. Which key is never seen.
//!
//! A key's release is input too, so key events are halved into key
//! presses.
//!
//! It's a guess (`InputKinds::Inferred`): clicks and scrolling without
//! moving the mouse count as key presses, and two inputs within one poll
//! count once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::InputCounts;

const POLL: Duration = Duration::from_millis(40);

/// One poll's reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// When the last input happened (ms, any steady clock).
    pub last_input_ms: u64,
    pub cursor: (i32, i32),
    /// A mouse button is held down right now.
    pub button_down: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Key,
    Mouse,
}

/// Turns consecutive probes into input events.
#[derive(Debug)]
pub struct Classifier {
    /// The last input time already counted, and the previous cursor.
    last: Option<(u64, (i32, i32))>,
    /// The last input time may wobble by this much between polls without
    /// any input (it's computed from an idle time and the current time).
    tolerance_ms: u64,
}

impl Classifier {
    pub fn new(tolerance_ms: u64) -> Self {
        Self {
            last: None,
            tolerance_ms,
        }
    }

    pub fn feed(&mut self, p: Probe) -> Option<Kind> {
        let Some((counted, cursor)) = self.last else {
            self.last = Some((p.last_input_ms, p.cursor));
            return None;
        };
        if p.last_input_ms > counted + self.tolerance_ms {
            let kind = if p.cursor != cursor || p.button_down {
                Kind::Mouse
            } else {
                Kind::Key
            };
            self.last = Some((p.last_input_ms, p.cursor));
            Some(kind)
        } else {
            self.last = Some((counted, p.cursor));
            None
        }
    }
}

/// What the thread counted: key events (presses and releases) and mouse
/// events.
#[derive(Debug, Default)]
struct Counted {
    key_events: u32,
    mouse: u32,
}

/// The polling thread and what it counted. Stops when dropped.
pub struct Inferred {
    counts: Arc<Mutex<Counted>>,
    stop: Arc<AtomicBool>,
}

impl Inferred {
    /// `read` gives one probe, or `None` if the OS couldn't answer.
    pub fn start(
        tolerance_ms: u64,
        mut read: impl FnMut() -> Option<Probe> + Send + 'static,
    ) -> Option<Self> {
        let counts = Arc::new(Mutex::new(Counted::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_counts, thread_stop) = (counts.clone(), stop.clone());
        std::thread::Builder::new()
            .name("input-kinds".into())
            .spawn(move || {
                let mut classifier = Classifier::new(tolerance_ms);
                while !thread_stop.load(Ordering::Relaxed) {
                    if let Some(kind) = read().and_then(|p| classifier.feed(p)) {
                        let mut c = thread_counts.lock().unwrap_or_else(|e| e.into_inner());
                        match kind {
                            Kind::Key => c.key_events = c.key_events.saturating_add(1),
                            Kind::Mouse => c.mouse = c.mouse.saturating_add(1),
                        }
                    }
                    std::thread::sleep(POLL);
                }
            })
            .ok()?;
        Some(Self { counts, stop })
    }

    /// What was counted since the last call. An odd key event (a press
    /// whose release comes later) waits for the next call.
    pub fn take(&self) -> InputCounts {
        let mut c = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        let keys = c.key_events / 2;
        c.key_events %= 2;
        InputCounts {
            keys,
            mouse: std::mem::take(&mut c.mouse),
        }
    }
}

impl Drop for Inferred {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(at: u64, cursor: (i32, i32), button_down: bool) -> Probe {
        Probe {
            last_input_ms: at,
            cursor,
            button_down,
        }
    }

    #[test]
    fn tells_keys_from_mouse() {
        let mut c = Classifier::new(15);
        assert_eq!(c.feed(probe(1000, (5, 5), false)), None);
        // Nothing new, only the clock wobbling.
        assert_eq!(c.feed(probe(1010, (5, 5), false)), None);
        // New input, cursor still: a key.
        assert_eq!(c.feed(probe(1100, (5, 5), false)), Some(Kind::Key));
        // New input with the cursor moved: mouse.
        assert_eq!(c.feed(probe(1200, (9, 5), false)), Some(Kind::Mouse));
        // A button down without moving: mouse.
        assert_eq!(c.feed(probe(1300, (9, 5), true)), Some(Kind::Mouse));
        assert_eq!(c.feed(probe(1300, (9, 5), false)), None);
    }

    #[test]
    fn wobble_doesnt_add_up() {
        let mut c = Classifier::new(15);
        c.feed(probe(1000, (0, 0), false));
        for at in [1010, 1005, 1012, 1014] {
            assert_eq!(c.feed(probe(at, (0, 0), false)), None);
        }
    }
}
