//! What an agent's edit tool calls claim to change, per file.
//!
//! Known tools: Claude Code's `Edit`, `MultiEdit` and `Write`, opencode's
//! `edit` and `write`. Other tools (shell commands, patches) aren't counted
//! here; their changes still show up in the working directory.

use std::path::Path;

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
