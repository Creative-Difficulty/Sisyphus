//! Watches a project folder for file changes while its agent runs, by
//! polling: every couple of seconds each file's size and modification time
//! are checked, and changed files are re-read and compared line by line.
//!
//! No file content is kept: each line is reduced to a hash of its
//! non-whitespace characters and how many there are. That's enough to count
//! non-whitespace characters added and removed, and nothing can be read
//! back from it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use sisyphus_common::heartbeat::Changes;

/// How often an armed watch checks the folder.
const POLL: Duration = Duration::from_secs(2);
/// Bigger files are skipped (generated or data files, not code).
const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// A folder with more files than this is only watched in part.
const MAX_FILES: usize = 20_000;
const MAX_DEPTH: usize = 32;
/// Folders that hold dependencies, build output or version control data.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".gradle",
    ".cache",
    "dist",
    "build",
    "out",
];

/// One line, reduced to what's needed for counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Line {
    /// Hash of the line's non-whitespace characters.
    hash: u64,
    /// How many non-whitespace characters it has.
    chars: u32,
}

/// The lines of `text` that have non-whitespace characters. Indentation
/// and spacing don't count, so re-indenting a line isn't a change.
pub fn lines_of(text: &str) -> Vec<Line> {
    text.lines()
        .filter_map(|line| {
            let mut hasher = DefaultHasher::new();
            let mut chars = 0u32;
            for c in line.chars().filter(|c| !c.is_whitespace()) {
                c.hash(&mut hasher);
                chars += 1;
            }
            (chars > 0).then(|| Line {
                hash: hasher.finish(),
                chars,
            })
        })
        .collect()
}

/// Non-whitespace characters of lines added and removed from `old` to
/// `new`. Lines that only moved within the changed region don't count.
pub fn diff(old: &[Line], new: &[Line]) -> Changes {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let (old, new) = (&old[prefix..], &new[prefix..]);
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old, new) = (&old[..old.len() - suffix], &new[..new.len() - suffix]);

    // What's left: lines in one but not the other, counted as multisets.
    let mut balance: HashMap<Line, i64> = HashMap::new();
    for line in old {
        *balance.entry(*line).or_default() += 1;
    }
    for line in new {
        *balance.entry(*line).or_default() -= 1;
    }
    let mut changes = Changes::default();
    for (line, n) in balance {
        let chars = u64::from(line.chars) * n.unsigned_abs();
        let chars = u32::try_from(chars).unwrap_or(u32::MAX);
        if n > 0 {
            changes.removed = changes.removed.saturating_add(chars);
        } else if n < 0 {
            changes.added = changes.added.saturating_add(chars);
        }
    }
    changes
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

struct File {
    stamp: Stamp,
    /// `None` for files that aren't text.
    lines: Option<Vec<Line>>,
    /// The lines before the latest change.
    previous: Option<Vec<Line>>,
}

#[derive(Default)]
struct State {
    armed: bool,
    files: HashMap<String, File>,
    /// Changes seen since the last [`ProjectWatch::take_changes`].
    changes: Vec<(String, Changes)>,
}

/// A polling watch on one project folder. Polls only while armed; the
/// first poll after arming takes a baseline without counting, so changes
/// made while it was disarmed don't count.
pub struct ProjectWatch {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ProjectWatch {
    pub fn start(root: PathBuf) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let state = state.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("project-watch".into())
                .spawn(move || run(&root, &state, &stop))
                .ok()
        };
        Self {
            state,
            stop,
            thread,
        }
    }

    pub fn set_armed(&self, armed: bool) {
        lock(&self.state).armed = armed;
    }

    /// Changes seen since the last call, by path relative to the root.
    pub fn take_changes(&self) -> Vec<(String, Changes)> {
        std::mem::take(&mut lock(&self.state).changes)
    }

    /// The file's lines before the agent wrote `written` to it: its
    /// previous version if the watch already saw the write, its current one
    /// if not. Empty for a file the watch doesn't know (a new file).
    pub fn lines_before_write(&self, path: &str, written: &[Line]) -> Vec<Line> {
        let state = lock(&self.state);
        let Some(file) = state.files.get(path) else {
            return Vec::new();
        };
        if file.lines.as_deref() == Some(written) {
            file.previous.clone().unwrap_or_default()
        } else {
            file.lines.clone().unwrap_or_default()
        }
    }
}

impl Drop for ProjectWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(root: &Path, state: &Mutex<State>, stop: &AtomicBool) {
    let mut baseline_taken = false;
    while !stop.load(Ordering::Relaxed) {
        if lock(state).armed {
            poll(root, state, baseline_taken);
            baseline_taken = true;
        } else {
            baseline_taken = false;
        }
        // Sleep in short steps so dropping the watch doesn't wait long.
        let mut slept = Duration::ZERO;
        while slept < POLL && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
            slept += Duration::from_millis(200);
        }
    }
}

/// Compare the folder with what's known; record changes if `count`.
fn poll(root: &Path, state: &Mutex<State>, count: bool) {
    let mut found = Vec::new();
    walk(root, root, 0, &mut found);

    let changed: Vec<(String, PathBuf, Stamp)> = {
        let state = lock(state);
        found
            .iter()
            .filter(|(rel, _, stamp)| state.files.get(rel).is_none_or(|f| f.stamp != *stamp))
            .cloned()
            .collect()
    };
    for (rel, path, stamp) in changed {
        let lines = read_lines(&path, stamp);
        let mut state = lock(state);
        let old = state.files.remove(&rel);
        let change = match (&old, &lines) {
            (Some(old), Some(new)) => old.lines.as_deref().map(|old| diff(old, new)),
            (None, Some(new)) => Some(diff(&[], new)),
            _ => None,
        };
        if count && let Some(change) = change.filter(|c| !c.is_empty()) {
            state.changes.push((rel.clone(), change));
        }
        let previous = old.and_then(|f| f.lines);
        state.files.insert(
            rel,
            File {
                stamp,
                lines,
                previous,
            },
        );
    }

    // Deleted files.
    let present: std::collections::HashSet<&str> =
        found.iter().map(|(rel, ..)| rel.as_str()).collect();
    let mut state = lock(state);
    let gone: Vec<String> = state
        .files
        .keys()
        .filter(|rel| !present.contains(rel.as_str()))
        .cloned()
        .collect();
    for rel in gone {
        if let Some(lines) = state.files.remove(&rel).and_then(|f| f.lines)
            && count
        {
            let change = diff(&lines, &[]);
            if !change.is_empty() {
                state.changes.push((rel, change));
            }
        }
    }
}

/// The text files under `dir`: path relative to `root` (with `/`), full
/// path and stamp. Symlinks aren't followed.
fn walk(root: &Path, dir: &Path, depth: usize, found: &mut Vec<(String, PathBuf, Stamp)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if found.len() >= MAX_FILES {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            let name = entry.file_name();
            if !SKIP_DIRS.iter().any(|skip| name == *skip) {
                walk(root, &path, depth + 1, found);
            }
        } else if kind.is_file()
            && let Ok(meta) = entry.metadata()
            && meta.len() <= MAX_FILE_SIZE
            && let Some(rel) = relative(root, &path)
        {
            let stamp = Stamp {
                len: meta.len(),
                modified: meta.modified().ok(),
            };
            found.push((rel, path, stamp));
        }
    }
}

/// `path` relative to `root`, with `/` separators. `None` outside `root`.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for part in rel.components() {
        match part {
            std::path::Component::Normal(name) => parts.push(name.to_str()?.to_string()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The file's lines, or `None` if it isn't text or can't be read.
fn read_lines(path: &Path, stamp: Stamp) -> Option<Vec<Line>> {
    if stamp.len > MAX_FILE_SIZE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(lines_of(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(old: &str, new: &str) -> (u32, u32) {
        let c = diff(&lines_of(old), &lines_of(new));
        (c.added, c.removed)
    }

    #[test]
    fn counts_non_whitespace_characters() {
        assert_eq!(changes("", "fn main() {}\n"), (10, 0));
        assert_eq!(changes("a b c\n", ""), (0, 3));
        assert_eq!(changes("let x = 1;\n", "let x = 2;\n"), (7, 7));
    }

    #[test]
    fn whitespace_changes_dont_count() {
        assert_eq!(
            changes("fn a() {\nx();\n}\n", "fn a() {\n    x();\n}\n\n\n"),
            (0, 0)
        );
    }

    #[test]
    fn unchanged_lines_around_an_edit_dont_count() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nb\nXY\nc\nd\n";
        assert_eq!(changes(old, new), (2, 0));
    }

    #[test]
    fn relative_paths() {
        let root = Path::new("/home/u/project");
        assert_eq!(
            relative(root, Path::new("/home/u/project/src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(relative(root, Path::new("/home/u/other/x.rs")), None);
        assert_eq!(relative(root, Path::new("/home/u/project")), None);
    }

    #[test]
    fn watch_counts_changes_after_the_baseline() {
        let dir = std::env::temp_dir().join(format!("sisyphus-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "one\n").unwrap();
        let state = Mutex::new(State::default());

        poll(&dir, &state, false);
        assert!(lock(&state).changes.is_empty());

        std::fs::write(dir.join("src/a.rs"), "one\ntwo three\n").unwrap();
        std::fs::write(dir.join("src/b.rs"), "new\n").unwrap();
        std::fs::write(dir.join("target/out.rs"), "ignored\n").unwrap();
        poll(&dir, &state, true);
        let mut seen = std::mem::take(&mut lock(&state).changes);
        seen.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            seen,
            vec![
                (
                    "src/a.rs".to_string(),
                    Changes {
                        added: 8,
                        removed: 0
                    }
                ),
                (
                    "src/b.rs".to_string(),
                    Changes {
                        added: 3,
                        removed: 0
                    }
                ),
            ]
        );

        std::fs::remove_file(dir.join("src/b.rs")).unwrap();
        poll(&dir, &state, true);
        assert_eq!(
            lock(&state).changes,
            vec![(
                "src/b.rs".to_string(),
                Changes {
                    added: 0,
                    removed: 3
                }
            )]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
