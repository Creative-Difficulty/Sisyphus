# Data gathering with a terminal agent

How to generate realistic session data from any terminal coding agent (Codex,
Claude Code, opencode, …) so Sisyphus integrations can be built and tested
against it. When the user says "go" (optionally naming the agent), follow this
document start to finish without further questions.

## Goal

Produce at least one hour of real, varied agent sessions on the local machine,
covering every kind of record the agent writes: prompts (typed and pasted),
replies, reasoning, tool calls and results (success, failure, rejected),
interrupts, turn ends, compaction, renames, slash commands, resumed and
parallel sessions. Sisyphus tracks passively, so the data just has to exist in
the agent's own files; nothing is installed into the agent.

## Rules

- **Never automate the session.** You, the model, are the user. Type every
  prompt yourself with `tmux send-keys`, look at the screen yourself with
  `tmux capture-pane`, and decide every approval yourself. No driver scripts,
  no prompt queues, no wait loops, no auto-approval. The point is to explore
  what data the harness produces, which needs your attention at every step.
- **Be non-linear.** The opposite of the goal is a neat ping-pong of "my
  prompt, its turn, my prompt, its turn". Mix things up without blowing
  everything up:
  - type a follow-up while the agent is still working (steering)
  - interrupt mid-reply or mid-tool, then continue, change topic or apologise
  - run two or more sessions in parallel and switch between them
  - reject or cancel approvals, contradict earlier instructions, ask it to
    undo things
  - send unexpected input: an empty Enter, a bare `/`, a typo'd command
  - quit mid-turn, resume later, open slash-command dialogs and back out
- **Run autonomously.** The user may be away. Don't stop between turns or wait
  for confirmation; keep going until the time and coverage goals are met.
  Check the screen and the agent's session files often to catch stuck
  sessions or mistakes yourself.
- **Choose the project yourself.** Pick a new, original idea each run, of
  moderate complexity, and grow it into a real code base: at least five
  distinct features and roughly 3000 lines. Business-style ideas with real
  domain rules work well (for example a piano studio's scheduler with
  billing, concerts and role-based dashboards). Change direction when it
  makes sense: ask for refactors, question the architecture, swap an
  approach, add a web UI on top of a CLI.
- **Act like a normal user.** Write casual prompts with the occasional typo,
  give context about who the software is for, ask real questions, praise good
  work, push back on bad decisions. Never mention testing, data collection,
  Sisyphus, or that a script is typing. The project folder name must look
  like a real project.
- **Stay light.** Prefer the standard library of one language (Python is
  fine). No heavy dependencies, no Docker, no large downloads. Small packages
  like `pytest` are fine.
- **Stay local and contained.** Work only in a new folder outside the
  Sisyphus repo (e.g. `~/Documents/Projects/<project-name>`). Never paste
  secrets. Don't commit or push anything in the Sisyphus repo as part of this.
- **Never print config files whole.** Agent configs can hold API keys (even
  in odd places like an auth command's `args`). Grep for the specific keys
  you need instead of printing sections, and never rely on a redaction
  regex.
- **Explore the harness itself.** Before and during the run, look up every
  feature the agent has (its `--help`, slash-command popup, docs or source):
  side questions, plan mode, sub-agents, forks, goals, exports, hooks. Use
  each at least once, ideally at an odd moment (a side question while the
  main thread works, a fork after a crash).
- **Use the agent's normal configuration** (`~/.codex`, `~/.claude`, …) unless
  the user says otherwise. Use a cheap model if the agent is configured for
  one; don't change the user's model or provider settings.
- **Duration:** at least 60 minutes of wall-clock time from the first prompt,
  with natural pauses (20–90 s, sometimes several minutes) between turns so
  the data contains both active and idle stretches.

## Setup

1. Create the project folder and `git init` it.
2. Start the agent in a detached tmux session sized like a real terminal, with
   the pane kept open if the agent exits, so errors stay visible:

   ```sh
   tmux new-session -d -s agent -x 180 -y 48 -c "$PROJECT"
   tmux set-option -t agent remain-on-exit on
   tmux respawn-pane -k -t agent "<agent command and flags>"
   ```

   Pick flags that make the agent ask for approval sometimes (Codex:
   `-a on-request -s workspace-write`), so approvals and rejections happen.
   Check `<agent> --help` first; accepted values change between versions.
3. Open a read-only live view for the user (macOS):

   ```sh
   osascript -e 'tell application "Terminal" to do script "tmux attach -r -t agent"'
   ```

   On Linux, open a terminal emulator running `tmux attach -r -t agent`.
4. Answer first-run dialogs (folder trust, onboarding) by reading the screen
   with `tmux capture-pane -p -t agent`.

## Driving the agent by hand

- **Typing:** `tmux send-keys -t '=<session>:' -l "<text>"`, then a separate
  `tmux send-keys -t '=<session>:' Enter` about a second later. Text sent in
  one burst looks like a paste to some agents (Codex), and an Enter in the
  same burst becomes a newline instead of submitting.
- **Targets:** always address tmux sessions exactly as `'=name:'`. A plain
  `-t codex` also matches `codex2` and sends keys to the wrong session.
- **Parallel sessions:** use one tmux *session* per agent session (not
  windows), each with its own read-only Terminal view. A read-only client
  (`attach -r`) can't switch windows, so the user couldn't see a second
  window.
- **Pasting:** `tmux load-buffer <file>` then `tmux paste-buffer -p` sends a
  bracketed paste, which agents treat as a real paste (large ones become a
  placeholder like `[Pasted Content N chars]`). Paste at least one long text
  (over 1000 characters): a spec, an error log or a stack trace.
- **Watching:** wait a few seconds with a short sleep, then
  `tmux capture-pane -p -t '=<session>:' | sed '/^\s*$/d' | tail -20`. Look at
  every session regularly; answer approvals as they appear.
- **Keys:** `Escape` interrupts or cancels a dialog, `C-u` clears the input
  line (e.g. the suggested name in a rename dialog), `BSpace` deletes.

## Coverage checklist

Work through all of these, spread naturally over the hour. Find out what the
specific agent offers (`/help`, `--help`, its docs) and add everything it has.

**Prompts**
- short typed prompts, long typed prompts, follow-up questions
- one or more large pastes (spec, traceback, log)
- correction prompts ("no, that's wrong, use a counter instead")
- questions that need only reading code, no edits
- vague requests the agent has to clarify
- a request that changes direction mid-project

**Tools**
- file reads, searches, edits and new files (the agent's patch/edit tool)
- shell commands that succeed, commands that fail (non-zero exit, missing
  file, syntax error in the code)
- running tests, fixing failing tests
- web search or fetch, if the agent has it
- planning/todo tools, sub-agents, MCP tools, image input, if available

**Approvals**
- approve a command once
- approve with "don't ask again" if offered
- reject a command ("No, continue without running it" if offered)
- cancel at an approval prompt and give a different instruction

**Interrupts**
- press Escape while the agent is writing a reply
- press Escape while a tool or command is running
- send a new prompt right after an interrupt

**Session features**
- slash commands: compact, rename, status, model or settings views, diff,
  review, new session, and anything else listed in the agent's help
- compaction (manual, and ideally automatic in a long session)
- a second session running in parallel in another tmux window, in a second
  project folder
- resume an earlier session (`codex resume`, `claude --resume`, …) and keep
  working in it
- fork or revert/rewind a session, if the agent supports it
- start a session, send one prompt and quit (short session)
- quit the agent mid-turn and restart it

**Unexpected behaviour**
- ask for something the sandbox blocks (network, writing outside the project)
- give contradictory instructions across turns
- ask it to undo its last change
- send an empty Enter or a prompt with only a slash
- very long running command (e.g. a slow test) and interrupt it

## Struggle signals to produce

Sisyphus wants to measure the user's productive struggle, so make sure the
data contains each of these several times (Codex record names in brackets):

- correction prompts and pushback ("no, that's wrong …") [`UserMessage`]
- interrupts, at different moments [`turn_aborted` reason `interrupted`;
  a preceding tool output "aborted by user" means during a tool]
- cancelled and declined approvals [cancel looks like an interrupt right
  after a `function_call` with no output; decline gives `status: declined`
  and "rejected by user"]
- steering: extra prompts sent while a turn runs [more than one
  `UserMessage` between `task_started` and `task_complete`]
- failing commands and fix loops, both agent-run and user-run with `!`
  [`CommandExecution` `status: failed`, `exit_code`, `source: user_shell`]
- pasted error logs and tracebacks [only detectable from the prompt text]
- re-asking nearly the same thing, asking to undo, rolling back
  [`thread_rolled_back`]
- reading time: pauses of different lengths before the next prompt
  [gap between `task_complete` and the next `task_started`]
- side questions, plan-mode answers with notes, reviews, model or effort
  switches [`/btw` threads, plan answers, `/review`, `turn_context`]

## Check-ins

Every few turns, and whenever a wait returns something unexpected:
- look at the screen; answer dialogs, recover from errors
- confirm the session file is growing and contains the expected record types
  (e.g. count `"type"` values in the newest session file)
- keep a running list of which checklist items are done

## Finishing

1. After at least 60 minutes and with the checklist covered, let the last turn
   finish, then quit the agent normally.
2. Leave the project folder and the agent's session files in place; they are
   the data. Kill the tmux session (`tmux kill-session -t agent`).
3. Report to the user in a few lines: duration, number of sessions and turns,
   which checklist items were covered, which weren't possible with this agent,
   and the approximate model cost if known.

## Agent notes

- **Codex** (0.171): sessions in `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`,
  titles in `session_index.jsonl`, index in `state_5.sqlite`. A turn end is a
  `task_complete` or `turn_aborted` line.
  - Approval flag `-a` accepts only `on-request` and `never`; use
    `-a on-request -s workspace-write -c sandbox_workspace_write.exclude_slash_tmp=true -c sandbox_workspace_write.exclude_tmpdir_env_var=true`.
    Without the last two, the sandbox lets the agent write scratch files to
    `/tmp` without asking (it left about 20 there in the first run). Check
    `/tmp` afterwards and delete what the run created. Each new folder first shows a "Trust
    this folder?" dialog (Enter).
  - Approval dialogs offer "Yes, proceed" (`y`), sometimes "don't ask again"
    (`p`), and "No, and tell Codex what to do differently" (`Escape`), which
    aborts the turn like an interrupt. Sandbox blocks (network, `/tmp`)
    trigger approval requests.
  - Messages typed while it works are accepted mid-turn.
  - Slash commands (full list in `codex-rs/tui/src/slash_command.rs`):
    `/status`, `/rename` (shows a generated suggestion; `C-u` to clear),
    `/compact`, `/recap`, `/model`, `/permissions`, `/diff`, `/review`,
    `/new`, `/clear`, `/resume`, `/fork`, `/archive`, `/delete`, `/export`,
    `/init` (writes AGENTS.md), `/plan`, `/goal`, `/btw` and `/side`,
    `/agents`, `/multi-agents`, `/mention`, `/cd`, `/ps`, `/stop`,
    `/rollout` (prints the session file path). Type `/` to see the popup.
  - `!<command>` in the input runs a shell command as the user (recorded as a
    user shell command, including failures).
  - `/btw` opens a side conversation in a split panel; `C-_` (Ctrl-/)
    switches between main thread and side panel. In a wide terminal the
    side panel is on the right, so capture with `cut -c<col>-` to read it.
  - Ctrl-C in the side panel asks "Archive …?" (`Right`, Enter to confirm).
    While the side panel or its dialog has focus, a main-thread approval
    can sit unseen and the turn stalls ("Working" without "esc to
    interrupt"). Check both panes; close the side panel when done.
  - `/quit` exits cleanly and prints the `codex resume <id>` command.
  - `/plan` (or Shift-Tab) switches to Plan mode, where the agent asks
    structured multiple-choice questions: arrow keys to pick, `Tab` to add a
    free-text note, `←`/`→` to move between questions, Enter to submit.
  - After a plan, "Yes, clear context and implement" starts a new thread
    whose first prompt Codex writes itself ("Read the approved plan at
    ~/.codex/attachments/…/approved-plan.md and implement it"): a user
    prompt no human typed.
  - Asking for sub-agents ("spawn a couple of subagents in parallel …")
    works; their approval prompts appear in the parent with `o` to open the
    sub-agent's thread. `/agents` opens a command center listing every
    thread on the machine, grouped by folder; don't open other projects'
    threads. `/multi-agents` was not recognised in 0.171.
  - While it works, Enter sends a message mid-turn (steering), `Tab` queues
    it for after the turn, `Shift-←` pulls the last queued message back for
    editing. Interrupting puts all queued messages back into the input box,
    merged into one.
  - `/cd <dir>` switches the same session to another folder (trust dialog
    first), so one session can span several projects.
  - `/model` then `s` changes model or effort for this session only.
  - `/export` → "Save to file" writes a Markdown transcript into the project
    folder (avoid "Copy to clipboard", it touches the OS clipboard).
  - `/archive` (confirm with Down, Enter) moves the thread and its
    sub-agents' files to `~/.codex/archived_sessions/` and drops to the
    command center.
  - Sub-agents may finish without the parent noticing; the parent then waits
    forever on "Waiting for <id>". Interrupt and tell it to carry on.
  - `/new` asks whether to use the current folder or create a new git
    worktree; pick the current folder (a worktree lands outside the project).
  - Ctrl-C does not quit while it works. To quit abruptly mid-turn, kill the
    process: `tmux respawn-pane -k -t '=<session>:' "<next command>"`.
  - `codex resume --last` failed with "model provider … is no longer
    configured" for a provider defined in `config.toml`; passing it
    explicitly (`-c model_provider=<name>`) works. After a hard kill, the
    resumed session is "open in another app" (locked); `r` retries, `f`
    forks it into a new thread.
  - DeepSeek models sometimes drift and answer an unasked question at the end
    of a turn; push back like a user would.
  - The terminal UI records its sessions with `source: "vscode"`, and writes
    the new `item_completed` format.
- **Claude Code**: transcripts in `~/.claude/projects/<escaped cwd>/*.jsonl`,
  prompt history in `~/.claude/history.jsonl`.
- **opencode**: SQLite database in `$XDG_DATA_HOME/opencode/opencode.db`,
  prompt history in `$XDG_STATE_HOME/opencode/prompt-history.jsonl`.
