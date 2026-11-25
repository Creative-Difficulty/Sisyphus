//! Sorting the agents' tool calls into coarse kinds for heartbeats.

use sisyphus_common::heartbeat::ToolKind;

/// The kind of a tool call by its tool's name, for every agent at once
/// (Claude Code, opencode, Codex). `edits` says whether the call claimed
/// file edits (Codex also edits through shell commands holding a patch).
/// Tools it doesn't know, MCP tools among them, are `Other`.
pub fn kind_of(name: &str, edits: bool) -> ToolKind {
    if edits {
        return ToolKind::Edit;
    }
    match name.to_ascii_lowercase().as_str() {
        // Claude Code, opencode, Codex.
        "bash" | "bashoutput" | "killshell" | "killbash" | "shell" | "exec_command"
        | "shell_command" | "local_shell" | "container.exec" | "write_stdin" => ToolKind::Shell,
        "edit" | "multiedit" | "write" | "notebookedit" | "patch" | "apply_patch" => ToolKind::Edit,
        "read" | "glob" | "grep" | "ls" | "list" | "notebookread" | "read_file" | "list_dir"
        | "grep_files" | "view_image" => ToolKind::Read,
        "webfetch" | "websearch" | "web_fetch" | "web_search" | "codesearch" => ToolKind::Web,
        "task" | "agent" | "spawn_agent" => ToolKind::Agent,
        _ => ToolKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        for (name, kind) in [
            ("Bash", ToolKind::Shell),
            ("exec_command", ToolKind::Shell),
            ("Edit", ToolKind::Edit),
            ("apply_patch", ToolKind::Edit),
            ("Grep", ToolKind::Read),
            ("read", ToolKind::Read),
            ("WebFetch", ToolKind::Web),
            ("web_search", ToolKind::Web),
            ("Task", ToolKind::Agent),
            ("TodoWrite", ToolKind::Other),
            ("mcp__slack__send", ToolKind::Other),
        ] {
            assert_eq!(kind_of(name, false), kind, "{name}");
        }
        // A shell command that carries a patch edits.
        assert_eq!(kind_of("exec_command", true), ToolKind::Edit);
    }
}
