//! What an agent's edit tool calls claim to change, per file.
//!
//! Known tools: Claude Code's `Edit`, `MultiEdit` and `Write`, opencode's
//! `edit` and `write`, and Codex's patches (see [`patch_claims`]). Other
//! tools (plain shell commands) aren't counted here; their changes still
//! show up in the working directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sisyphus_common::heartbeat::Changes;

use super::files::{Line, diff, lines_of, relative};

/// One file a tool call claims to change.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    /// Relative to the project folder, with `/` separators.
    pub path: String,
    pub kind: ClaimKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimKind {
    /// Replace text: the counts follow from the call alone.
    Replace(Changes),
    /// Write the whole file: the counts depend on what was there before.
    Write(Vec<Line>),
}

/// The files a tool call claims to change. Paths outside `project` are
/// dropped.
pub fn claims_of(tool: &str, input: &Value, project: &Path) -> Vec<Claim> {
    // Codex edits through patches, either with its own tool or inside a
    // shell command. Only Codex's tools are read, so another agent's shell
    // command that merely mentions a patch doesn't claim anything.
    if CODEX_PATCH_TOOLS.contains(&tool) {
        return find_patch(input)
            .map(|(patch, cd)| patch_claims(patch, &base_dir(project, cd)))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(path, kind)| {
                Some(Claim {
                    path: claim_path(project, &path)?,
                    kind,
                })
            })
            .collect();
    }
    let text = |keys: &[&str]| -> Option<&str> {
        keys.iter()
            .find_map(|k| input.get(*k).and_then(Value::as_str))
    };
    let Some(path) = text(&["file_path", "filePath"]).and_then(|p| claim_path(project, p)) else {
        return Vec::new();
    };
    let replace = |edit: &Value| -> Changes {
        let get = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| edit.get(*k).and_then(Value::as_str))
                .unwrap_or("")
        };
        diff(
            &lines_of(get(&["old_string", "oldString"])),
            &lines_of(get(&["new_string", "newString"])),
        )
    };
    let kind = match tool.to_ascii_lowercase().as_str() {
        "edit" => ClaimKind::Replace(replace(input)),
        "multiedit" => {
            let mut total = Changes::default();
            for edit in input
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                total.add(replace(edit));
            }
            ClaimKind::Replace(total)
        }
        "write" => ClaimKind::Write(lines_of(text(&["content"]).unwrap_or(""))),
        _ => return Vec::new(),
    };
    vec![Claim { path, kind }]
}

/// Codex's tools that can carry a patch: its own `apply_patch` (freeform,
/// or JSON with an `input` field in older versions) and its shell tools,
/// whose commands may run `apply_patch <<'EOF' … EOF`.
const CODEX_PATCH_TOOLS: &[&str] = &[
    "apply_patch",
    "exec_command",
    "shell",
    "shell_command",
    "local_shell",
];

const BEGIN_PATCH: &str = "*** Begin Patch";
const END_PATCH: &str = "*** End Patch";

/// The first patch in a tool input, and the folder a shell command `cd`s
/// into before applying it (`cd <dir> && apply_patch <<'EOF'`), if any.
fn find_patch(input: &Value) -> Option<(&str, Option<&str>)> {
    match input {
        Value::String(s) => {
            let begin = s.find(BEGIN_PATCH)?;
            let patch = &s[begin..];
            let end = patch
                .find(END_PATCH)
                .map_or(patch.len(), |e| e + END_PATCH.len());
            Some((&patch[..end], cd_prefix(&s[..begin])))
        }
        Value::Array(items) => items.iter().find_map(find_patch),
        Value::Object(fields) => fields.values().find_map(find_patch),
        _ => None,
    }
}

/// `dir` in a command starting with `cd dir && …`, without quotes.
fn cd_prefix(command: &str) -> Option<&str> {
    let rest = command.trim_start().strip_prefix("cd ")?;
    let dir = rest[..rest.find("&&")?].trim();
    let dir = dir
        .strip_prefix('\'')
        .and_then(|d| d.strip_suffix('\''))
        .or_else(|| dir.strip_prefix('"').and_then(|d| d.strip_suffix('"')))
        .unwrap_or(dir);
    (!dir.is_empty()).then_some(dir)
}

/// The folder a patch's relative paths are relative to.
fn base_dir(project: &Path, cd: Option<&str>) -> PathBuf {
    match cd {
        Some(dir) => project.join(dir), // an absolute `dir` replaces `project`
        None => project.to_path_buf(),
    }
}

/// What a Codex patch claims, per file, in the order the files first
/// appear. The format (`codex-rs/core/assets/tools/apply_patch.lark`):
/// `*** Begin Patch`, then per file `*** Add File: <path>` with `+` lines,
/// `*** Delete File: <path>`, or `*** Update File: <path>` with an optional
/// `*** Move to: <path>` and hunks of `@@` context lines and ` `/`-`/`+`
/// lines, then `*** End Patch`. Like Codex's own parser, whitespace around
/// the markers is allowed and unknown `***` lines are skipped. Relative
/// paths are joined to `base`.
pub fn patch_claims(patch: &str, base: &Path) -> Vec<(String, ClaimKind)> {
    enum Op {
        Add(Vec<String>),
        Update {
            removed: Vec<String>,
            added: Vec<String>,
        },
    }
    // Files in first-seen order, merged when a patch names one twice.
    let mut order: Vec<String> = Vec::new();
    let mut files: BTreeMap<String, ClaimKind> = BTreeMap::new();
    let mut current: Option<(String, Op)> = None;
    let mut finish = |current: &mut Option<(String, Op)>| {
        let Some((path, op)) = current.take() else {
            return;
        };
        let kind = match op {
            Op::Add(lines) => ClaimKind::Write(lines_of(&lines.join("\n"))),
            Op::Update { removed, added } => ClaimKind::Replace(diff(
                &lines_of(&removed.join("\n")),
                &lines_of(&added.join("\n")),
            )),
        };
        match (files.get_mut(&path), kind) {
            (Some(ClaimKind::Replace(total)), ClaimKind::Replace(more)) => total.add(more),
            (Some(existing), kind) => *existing = kind,
            (None, kind) => {
                order.push(path.clone());
                files.insert(path, kind);
            }
        }
    };
    let joined = |raw: &str| base.join(raw.trim()).to_string_lossy().into_owned();
    for line in patch.lines() {
        let marker = line.trim();
        if let Some(path) = marker.strip_prefix("*** Add File:") {
            finish(&mut current);
            current = Some((joined(path), Op::Add(Vec::new())));
        } else if let Some(path) = marker.strip_prefix("*** Update File:") {
            finish(&mut current);
            current = Some((
                joined(path),
                Op::Update {
                    removed: Vec::new(),
                    added: Vec::new(),
                },
            ));
        } else if let Some(path) = marker.strip_prefix("*** Delete File:") {
            finish(&mut current);
            current = Some((joined(path), Op::Add(Vec::new())));
            finish(&mut current); // a write of nothing
        } else if let Some(path) = marker.strip_prefix("*** Move to:") {
            if let Some((target, _)) = current.as_mut() {
                *target = joined(path);
            }
        } else if marker == END_PATCH {
            break;
        } else if marker.starts_with("***") || marker.starts_with("@@") {
            // Begin Patch, End of File, Environment ID, hunk context.
        } else if let Some((_, op)) = current.as_mut() {
            match (op, line.chars().next()) {
                (Op::Add(lines), Some('+')) => lines.push(line[1..].to_string()),
                (Op::Update { added, .. }, Some('+')) => added.push(line[1..].to_string()),
                (Op::Update { removed, .. }, Some('-')) => removed.push(line[1..].to_string()),
                _ => {} // context lines
            }
        }
    }
    finish(&mut current);
    order
        .into_iter()
        .filter_map(|path| files.remove(&path).map(|kind| (path, kind)))
        .collect()
}

/// A tool's file path relative to the project. Relative paths are taken
/// as relative to the project folder.
fn claim_path(project: &Path, raw: &str) -> Option<String> {
    let path = Path::new(raw);
    if path.is_absolute() {
        relative(project, path).or_else(|| {
            let canonical = project.canonicalize().ok()?;
            relative(&canonical, path)
        })
    } else {
        relative(project, &project.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn replace(added: u32, removed: u32) -> ClaimKind {
        ClaimKind::Replace(Changes { added, removed })
    }

    #[test]
    fn claude_code_edits() {
        let project = Path::new("/p");
        let edit = json!({"file_path": "/p/src/a.rs", "old_string": "x = 1;", "new_string": "x = 22;\ny();"});
        assert_eq!(
            claims_of("Edit", &edit, project),
            vec![Claim {
                path: "src/a.rs".into(),
                kind: replace(9, 4)
            }]
        );
        let multi = json!({"file_path": "/p/a", "edits": [
            {"old_string": "a", "new_string": "bb"},
            {"old_string": "", "new_string": "ccc"},
        ]});
        assert_eq!(
            claims_of("MultiEdit", &multi, project)[0].kind,
            replace(5, 1)
        );
        let write = json!({"file_path": "/p/new.md", "content": "# Hi\n"});
        assert_eq!(
            claims_of("Write", &write, project)[0].kind,
            ClaimKind::Write(lines_of("#Hi"))
        );
    }

    #[test]
    fn opencode_edits() {
        let edit = json!({"filePath": "/p/main.rs", "oldString": "a", "newString": "b"});
        assert_eq!(
            claims_of("edit", &edit, Path::new("/p")),
            vec![Claim {
                path: "main.rs".into(),
                kind: replace(1, 1)
            }]
        );
    }

    fn claim(path: &str, kind: ClaimKind) -> Claim {
        Claim {
            path: path.into(),
            kind,
        }
    }

    #[test]
    fn codex_patch_tool() {
        let project = Path::new("/p");
        let patch = "*** Begin Patch\n\
                     *** Add File: notes.md\n\
                     +# Notes\n\
                     +hello world\n\
                     *** Update File: src/a.rs\n\
                     @@ fn main() {\n\
                     -    old();\n\
                     +    new_call();\n\
                     \x20   keep();\n\
                     @@\n\
                     +    more();\n\
                     *** End of File\n\
                     *** Delete File: gone.txt\n\
                     *** End Patch";
        assert_eq!(
            claims_of("apply_patch", &json!(patch), project),
            vec![
                claim(
                    "notes.md",
                    ClaimKind::Write(lines_of("# Notes\nhello world"))
                ),
                claim("src/a.rs", replace(18, 6)),
                claim("gone.txt", ClaimKind::Write(Vec::new())),
            ]
        );
        // Older versions sent the patch as JSON with an `input` field.
        assert_eq!(
            claims_of("apply_patch", &json!({"input": patch}), project).len(),
            3
        );
    }

    #[test]
    fn codex_patch_inside_a_shell_command() {
        let project = Path::new("/p");
        let script = "cd /p/sub && apply_patch <<'PATCH'\n*** Begin Patch\n*** Update File: x.py\n-a\n+bb\n*** End Patch\nPATCH";
        let expected = vec![claim("sub/x.py", replace(2, 1))];
        assert_eq!(
            claims_of("exec_command", &json!({"cmd": script}), project),
            expected
        );
        assert_eq!(
            claims_of(
                "shell",
                &json!({"command": ["bash", "-lc", script]}),
                project
            ),
            expected
        );
        let relative_cd = "cd 'sub' && apply_patch <<'EOF'\n*** Begin Patch\n*** Update File: x.py\n-a\n+bb\n*** End Patch\nEOF";
        assert_eq!(
            claims_of("shell_command", &json!({"command": relative_cd}), project),
            expected
        );
        // A shell command without a patch claims nothing.
        assert!(claims_of("exec_command", &json!({"cmd": "ls"}), project).is_empty());
    }

    #[test]
    fn codex_patch_details() {
        let base = Path::new("/p");
        // Whitespace around markers, an environment id, a move, and a file
        // updated twice in one patch.
        let patch = "  *** Begin Patch\n\
                     *** Environment ID: local\n\
                     *** Update File: old.rs\n\
                     *** Move to: new.rs\n\
                     -a\n\
                     +b\n\
                     *** Update File: new.rs\n\
                     +cc\n\
                     *** End Patch  ";
        assert_eq!(
            patch_claims(patch, base),
            vec![("/p/new.rs".to_string(), replace(3, 1))]
        );
    }

    #[test]
    fn patches_from_other_agents_or_outside_the_project_are_ignored() {
        let project = Path::new("/p");
        let patch = "*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch";
        // Claude Code's and opencode's shell tools aren't read for patches.
        assert!(claims_of("Bash", &json!({"command": patch}), project).is_empty());
        assert!(claims_of("bash", &json!({"command": patch}), project).is_empty());
        let outside =
            "*** Begin Patch\n*** Add File: /etc/x\n+x\n*** Add File: ../y\n+y\n*** End Patch";
        assert!(claims_of("apply_patch", &json!(outside), project).is_empty());
    }

    #[test]
    fn other_tools_and_outside_paths_are_ignored() {
        let project = Path::new("/p");
        assert!(claims_of("Bash", &json!({"command": "ls"}), project).is_empty());
        let outside = json!({"file_path": "/etc/hosts", "content": "x"});
        assert!(claims_of("Write", &outside, project).is_empty());
        let escape = json!({"filePath": "../x", "content": "x"});
        assert!(claims_of("write", &escape, project).is_empty());
    }
}
