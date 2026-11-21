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
/// new. Bytes that aren't valid UTF-8 are replaced, so one bad line doesn't
/// end the follow.
pub fn follow_lines(
    path: &Path,
    stop: &AtomicBool,
    on_line: impl FnMut(&str, bool) -> bool,
) -> io::Result<()> {
    follow_lines_with_idle(path, stop, on_line, || true)
}

/// [`follow_lines`], also calling `on_idle` once per poll while waiting for
/// new lines. Following stops when it returns `false`.
pub fn follow_lines_with_idle(
    path: &Path,
    stop: &AtomicBool,
    mut on_line: impl FnMut(&str, bool) -> bool,
    mut on_idle: impl FnMut() -> bool,
) -> io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut history_end = reader.get_ref().metadata()?.len();
    let mut pos: u64 = 0;
    let mut buf: Vec<u8> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        let n = reader.read_until(b'\n', &mut buf)?;
        if n > 0 && buf.ends_with(b"\n") {
            pos += buf.len() as u64;
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim_end_matches(['\n', '\r']);
            if !line.is_empty() && !on_line(line, pos <= history_end) {
                return Ok(());
            }
            buf.clear();
            continue;
        }

        // At the end of the file, possibly holding a partial line that is
        // still being written. Keep it in `buf` and wait for the rest.
        if !on_idle() {
            return Ok(());
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("sisyphus-tail-{name}-{}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn invalid_utf8_does_not_stop_following() {
        let path = temp_file("utf8", b"one\n\xff\xfe\nthree\n");
        let stop = AtomicBool::new(false);
        let mut lines = Vec::new();
        follow_lines(&path, &stop, |line, history| {
            lines.push((line.to_string(), history));
            lines.len() < 3
        })
        .unwrap();
        assert_eq!(lines[0], ("one".to_string(), true));
        assert_eq!(lines[1], ("\u{fffd}\u{fffd}".to_string(), true));
        assert_eq!(lines[2], ("three".to_string(), true));
    }

    #[test]
    fn idle_runs_while_waiting_and_can_stop() {
        let path = temp_file("idle", b"old\n");
        let stop = AtomicBool::new(false);
        let mut lines = Vec::new();
        let mut idles = 0;
        follow_lines_with_idle(
            &path,
            &stop,
            |line, history| {
                lines.push((line.to_string(), history));
                true
            },
            || {
                idles += 1;
                if idles == 2 {
                    let mut f = File::options().append(true).open(&path).unwrap();
                    f.write_all(b"new\n").unwrap();
                }
                idles < 4
            },
        )
        .unwrap();
        assert_eq!(
            lines,
            [("old".to_string(), true), ("new".to_string(), false)]
        );
        assert_eq!(idles, 4);
    }
}
