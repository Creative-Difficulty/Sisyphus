//! Follow a growing line-based file (like `tail -f`).

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Call `on_line` for every complete line in `path`, then keep waiting for
/// new lines until `stop` is set or `on_line` returns `false`.
///
/// The second argument to `on_line` is true for lines that were already in
/// the file when following started. If the file shrinks (it was replaced or
/// truncated) it is read again from the start, with those lines counted as
/// new.
pub fn follow_lines(
    path: &Path,
    stop: &AtomicBool,
    mut on_line: impl FnMut(&str, bool) -> bool,
) -> io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut history_end = reader.get_ref().metadata()?.len();
    let mut pos: u64 = 0;
    let mut buf = String::new();

    while !stop.load(Ordering::Relaxed) {
        let n = reader.read_line(&mut buf)?;
        if n > 0 && buf.ends_with('\n') {
            pos += buf.len() as u64;
            let line = buf.trim_end_matches(['\n', '\r']);
            if !line.is_empty() && !on_line(line, pos <= history_end) {
                return Ok(());
            }
            buf.clear();
            continue;
        }

        // At the end of the file, possibly holding a partial line that is
        // still being written. Keep it in `buf` and wait for the rest.
        let len = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Deleted. Wait in case it is being replaced.
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            Err(e) => return Err(e),
        };
        if len < pos + buf.len() as u64 {
            reader = BufReader::new(File::open(path)?);
            pos = 0;
            history_end = 0;
            buf.clear();
            continue;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}
