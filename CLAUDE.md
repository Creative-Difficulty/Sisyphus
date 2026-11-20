# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Sisyphus is an open-source (AGPLv3) passive time tracker in Rust. It measures the hours of "productive struggle" that teenagers spend building projects with AI. It is the team's entry for the Hack Club AI time tracking bounty. The server is the source of truth: reviewers read what the server stored, never the client.

## Rules

- Only implement what the user explicitly asked for in the current conversation. Plans and ideas are context, not tasks. If a request is ambiguous, ask.
- Privacy: never collect or upload file contents, file paths, window titles, keystroke content or prompt text. Only counts, timings and coarse app classes. AI chat history and prompts stay local. (`AgentEvent`s carry full prompt and response text in memory on the client; anything that leaves the machine must be reduced to counts and timings.)
- Never write to, commit to or modify a user's repository. Everything runs in the background.
- Never credit more time than was actually tracked. Multipliers from 0.0 to 1.0 are applied server-side.
- Never trust the client clock for anything audited. The server timestamps.
- No new dependencies without a one-line justification and the user's approval.
- Keep Windows, macOS and Linux working, and keep the client light (low-spec machines).
- Never commit unless the user asks.

## Commands

```sh
cargo build                                  # whole workspace
cargo test --workspace                       # all tests
cargo test -p sisyphus-common --test chain   # one integration test file
cargo test -p sisyphus-client opencode::session::tests::abort_is_an_interrupt  # one test, by path filter
cargo clippy --workspace --all-targets
cargo fmt -p sisyphus-client                 # crates/common is not rustfmt-clean yet; don't reformat it unasked

cargo run -p sisyphus-client -- sessions                      # list Claude Code / opencode sessions
cargo run -p sisyphus-client -- watch [SESSION_ID] [--replay] # print a session's events live
cargo run -p sisyphus-client                                  # open the window
```

No `rust-toolchain` file or CI config exists. Edition 2024, resolver 3. Dependencies are declared in `[workspace.dependencies]` in the root `Cargo.toml` and referenced with `.workspace = true`.

## Architecture

Three crates in `crates/`:

- **`sisyphus-common`**: types shared by client and server. Data flow (see `lib.rs` docs): collectors produce `Heartbeat`s → the client signs them into a per-device hash chain (`chain.rs`: sequence number, previous hash, SHA-256 with domain tag `sisyphus/heartbeat/v1`, ed25519 signature) and queues them offline → batches are uploaded (`api.rs`); the server verifies the batch continues the stored `ChainTip`, stamps its own receive time, and flags heartbeats received more than 5 min after their claimed time as offline. Heartbeats are hashed and signed, so they must serialize deterministically: no floats, no maps, and no contents, paths or titles. Changing the hashed format means bumping the domain tag. The tamper-detection tests in `crates/common/tests/` must keep passing.
- **`sisyphus-client`**: a library (`lib.rs`: `agents`, `config`, `gui`) plus a binary (`main.rs`). Run without arguments it opens the iced window (`gui/`); `sessions` and `watch` are CLI commands over the same integrations. It does not yet use the chain or upload anything.
  - `config.rs` stores local settings as JSON in the OS config directory (`~/.config/sisyphus/config.json` on Linux), currently only `connected_agents`, a list of `Integration::id()`s.
  - The window starts with agent cards (logo, name, Connect). Connect calls the integration's `install()` if `is_installed()` is false, then saves it to the config. Logos are SVGs in `crates/client/assets/`, embedded with `include_bytes!`.
- **`sisyphus-server`**: an empty `main`.

### Heartbeat design (decided 2026-11-20, not implemented yet)

The team decided on this design. `heartbeat.rs` still has the older shape (WakaTime-style gaps, free-form `app` string). Only change it when the user asks.

- One heartbeat per minute per user while an agent is running or the user is working on a prompt. The server credits at most one minute per real minute, even with several agents or devices.
- The client sends only integer observations, never a 0.0–1.0 value of its own:
  - which of the minute's six 10-second slots had input while the agent's terminal or editor was focused
  - seconds focused on the agent's app vs other apps, by app class only (terminal / editor / browser / other)
  - whether the agent was running
  - prompt characters submitted, and how many were pasted
- The server computes attention and the 0.0–1.0 multiplier from these. The client runs the same formula only for display and to compare against the server.
- Attention resets to 1.0 on input. It drops slowly while the agent's window stays focused and quickly in other apps. It is computed across minutes on the server.
- No keylogging and no root. Input is detected only through OS idle-time APIs:
  - Windows: `GetLastInputInfo`
  - macOS: `CGEventSourceSecondsSinceLastEventType`
  - Linux X11: XScreenSaver extension
  - Linux Wayland: `ext-idle-notify`; there is no standard way to read the focused app, so that part is per compositor and still open.
- Not possible with this design: detecting deleting or rewriting inside a prompt draft.

### Agent integrations (`crates/client/src/agents/`)

`mod.rs` defines the plugin interface:
- `AgentIntegration` discovers sessions (`get_instances`). Its signature was written by hand; extend it rather than restructuring it.
- `AgentInstance` is one session. `read_events()` returns its history. `attach(AttachMode)` returns an `Attachment`, which wraps a worker thread and an mpsc channel of `AgentEvent`s; dropping it stops the worker.
- `Integration` is an enum over all integrations (`Integration::ALL`), with boxed-trait helpers so callers can treat every agent the same.
- A new agent needs a submodule, an `Integration` variant, and the match arms in `Integration`'s helpers.

Neither integration installs hooks. Both read the agent's own on-disk storage, whose format is undocumented: skip unknown lines and fields rather than erroring.

- **`claude_code/`** reads the append-only JSONL transcripts at `~/.claude/projects/<escaped cwd>/<session uuid>.jsonl` (or `$CLAUDE_CONFIG_DIR`).
  - `transcript.rs` maps lines to events. It filters `isMeta`, `isSidechain` and compaction-summary lines, and lines whose `origin.kind` is not human. Interrupts are `[Request interrupted by user…]` text. Rejected tool calls are recognised by `toolUseResult`.
  - Sessions are listed by scanning only the head and tail of each file, because transcripts reach hundreds of MB and title lines are re-appended.
  - Live following uses `tail.rs`, a polling tail.
- **`opencode/`** reads opencode v2's SQLite database (`$XDG_DATA_HOME/opencode/opencode.db`), read-only, using the `session_v2` and `session_message` tables.
  - The v1 `session`/`message`/`part` tables are legacy (already migrated) and are not read.
  - Assistant rows are rewritten in place while streaming. Attachments therefore poll by `time_updated` (`>=` watermark), and `session.rs` deduplicates by remembering which events it already emitted.

`Prompt.pasted_chars` comes from each agent's prompt history, not the session itself (`claude_code/history.rs` reads `~/.claude/history.jsonl`; `opencode/history.rs` reads `$XDG_STATE_HOME/opencode/prompt-history.jsonl`). Only large pastes are recorded there, so small pastes count as typed, and `None` means the prompt wasn't in the history (queued prompts, or opencode prompts older than its ~50-entry history). Only lengths are kept, never pasted text.
