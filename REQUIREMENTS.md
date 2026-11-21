# Sisyphus requirements and ideas

Everything the team has required, decided or floated, as of 2026-11-20.
This is context, not a task list: only build what the user asks for in the
current conversation.

## Bounty requirements (the final product)

Sisyphus is our entry for the Hack Club AI time tracking bounty. The final
product must:

1. Be open source under AGPLv3.
2. Accurately track the hours of productive struggle teenagers spend building
   projects with AI.
3. Be meaningful and motivating to use on its own, not just an anti-fraud
   tool.
4. Track time passively, with no manual start/stop. It must count long flow,
   a 5-minute session before school, and switching between several projects
   in an afternoon, each attributed correctly.
5. Have a central server with accounts, public-by-default profiles, projects,
   git integration, and a stats dashboard that updates in real time.
6. Be privacy-friendly enough that privacy-conscious users are comfortable
   installing it.
7. Work with all AI models and harnesses, the most popular first, through
   plugins.
8. Support Windows, macOS and Linux, and run on low-spec computers.
9. Be trustworthy: one submitted hour equals one real hour of productive
   struggle toward a shipped project.
10. Make submitted hours auditable by external parties. Logged and submitted
    hours may have different privacy guarantees.
11. Cost at most $500/month to host for 15,488 monthly active users logging
    109,759 hours/month.

## Hard rules

- Never credit more time than was actually tracked.
- Only counts, timings, coarse app classes, paths of edited files (relative to
  the project folder) and the agent's session titles leave the machine. Never
  prompt text, file contents, absolute paths, window titles or keystrokes.
  Paths and session titles are visible only to the user and reviewers, never
  on public profiles.
- The server's clock counts, never the client's.
- Never write to, commit to or modify a user's repository.
- No new dependencies without a short justification and the user's approval.
- Keep Windows, macOS and Linux working.

## Built

- The client reads each agent's own files and installs nothing into the agent:
  - Claude Code: session transcripts and `history.jsonl`.
  - opencode: its SQLite database (v2 tables) and `prompt-history.jsonl`.
- One plugin per agent, behind the same interface.
- Events: prompt (with pasted characters), response, thinking, tool call and
  result (done / failed / rejected), interrupt, turn end, compaction, slash
  command, title change.
- Running detection: whether an agent is mid-turn.
- Pasted characters per prompt, from the prompt history. Agents only record
  large pastes, so small pastes count as typed.
- Activity on every OS, without a keylogger or root: time since the last
  input and the focused app's class, including whether the focused window
  runs the session's agent (matched through the process tree). Not in
  heartbeats yet.
  - Windows: `GetLastInputInfo`, the foreground window's process.
  - macOS: CoreGraphics idle time, `NSWorkspace`'s frontmost app.
  - Linux X11: XScreenSaver, `_NET_ACTIVE_WINDOW`.
  - Linux Wayland: `ext-idle-notify` (GNOME: Mutter's IdleMonitor); focus
    from Hyprland IPC, Sway IPC, the "Window Calls" GNOME Shell extension,
    a KWin script on KDE, or `wlr-foreign-toplevel-management`.
- Client window (iced):
  - an onboarding step asking to allow activity on every OS: required to
    count time, says exactly what is shared (the kind of app and whether the
    user was active) and what never is (typing, keys, mouse movements, app
    names, titles, screen); changeable in settings
  - settings to connect and disconnect agents
  - a dashboard of recent sessions with placeholder "Recorded today" figures
  - a pink light and dark theme that follows the OS
- `sisyphus-common`: an ed25519-signed hash chain of heartbeats with
  tamper-detection tests.
- Server (axum + Postgres):
  - accepts signed heartbeat batches, checks the chain and signatures, and
    stores them with its own receive time
  - a web dashboard in the client's style with a sidebar: Overview (tracked
    time and counts) and Projects (each project with its tracked time);
    tracked time and projects are placeholders until crediting exists
  - a Dockerfile for deployment
  - accounts via Hack Club Auth (OpenID Connect) for the website and the
    app: the server is the only OAuth client; the app logs in through it and
    links its device key to the account
  - uploads accepted only from devices linked to an account; each user sees
    only their own devices and heartbeats
  - No sessions, projects or credit yet.

## Decided, not built

### Heartbeat scheme (final, approved 2026-11-20)

- Time is counted and calculated per agent session. Each session sends its own
  heartbeat every minute while its agent runs or its prompt is being written.
- Heartbeats are signed into the hash chain, queued offline and uploaded about
  every 30 seconds. The device id is sent once per upload batch.
- No 0–1 value, attention, struggle value or session UUID comes from the
  client; the server computes those, and the client computes the same values
  only to compare.
- Serialization stays deterministic: integers, booleans, short fixed strings
  and arrays in a fixed order, no floats, no maps. The format changes from v1,
  so the hash domain tag must be bumped.

```json
{
  "seq": 1042,
  "prev": "9f2c…",
  "hash": "4b1e…",
  "sig": "d07a…",
  "heartbeat": {
    "version": 2,
    "minute_ms": 1795201380000,

    "agent": "claude-code",
    "agent_session_id": "77ae4435-adc7-40d4-934f-f70f97ced5e4",
    "session_name": "Client traits for agent session events",
    "project": "Sisyphus",

    "agent_running": true,
    "activity": true,
    "slots": [
      { "input": true, "focus": "agent" },
      { "input": true, "focus": "agent" },
      { "input": false, "focus": "agent" },
      { "input": false, "focus": "browser" },
      { "input": true, "focus": "editor" },
      { "input": false, "focus": "editor" }
    ],

    "prompts": {
      "submitted": 1,
      "typed_chars": 86,
      "pasted_chars": 0,
      "corrections": 1
    },

    "responses": {
      "finished": 1,
      "chars": 4200
    },

    "files": [
      {
        "path": "CLAUDE.md",
        "working_dir": { "added": 200, "removed": 7 },
        "agent_edits": { "added": 193, "removed": 7 }
      }
    ]
  }
}
```

- `minute_ms`: the minute the client claims; the server checks it against its
  receive time.
- `agent`, `agent_session_id`: the server builds its session UUID from device
  id + agent + agent session id.
- `session_name`: the agent's own session title. Visible only to the user and
  reviewers.
- `project`: the working directory's folder name only.
- `activity` (added 2026-11-21): whether the user allowed activity reading.
  When `false`, the six slots are still sent, each `{ "input": false,
  "focus": "unknown" }`, so the server can tell "tracking not allowed" apart
  from "away".
- `slots`: the minute's six 10-second slots.
  - `input` is any keyboard or mouse input, read from the OS idle time; keys
    are never recorded.
  - `focus` is one of `agent` (the agent's own window), `terminal`, `editor`,
    `browser`, `other` or `none`: the app that held focus for most of the
    slot, so a short flash into another window doesn't change it. `unknown`
    when activity isn't allowed, or the system can't tell (for example GNOME
    without the Window Calls extension).
- `prompts`: counts only, never text.
  - `corrections` counts prompts the local model labelled as corrections.
- `responses` (added 2026-11-21): counts only, never text.
  - `finished`: agent responses that finished this minute.
  - `chars`: characters of their visible response text (not thinking, not
    tool calls). Lets the server check reading time: a human can't sensibly
    answer a long response seconds after it ends.
- `files`: one entry per file edited this minute, sorted by path.
  - `path` is relative to the project folder.
  - `working_dir` counts non-whitespace characters added and removed, as
    actually seen in the file while the agent runs.
  - `agent_edits` is what the agent's edit calls claim for that file.
- Input is detected without a keylogger and without root: OS "time since last
  input" APIs plus the class of the focused app.
- Reading activity needs the user's permission on every OS (decided
  2026-11-21): an onboarding step says it is required to count time and
  explains exactly what is shared. On Wayland, decided 2026-11-21: use the
  most cross-compositor way that needs no root (see Built). Root would be
  acceptable behind a clear popup, but nothing needs it: root only adds raw
  `/dev/input` access, which is keylogger-level access.
- GNOME doesn't expose the focused window. For now it is read through the
  third-party "Window Calls" extension when installed, otherwise unknown.
  Shipping our own GNOME Shell extension is for later.
- The agent's edit calls and the working directory's changes must match up,
  file by file.
- A prompt submitted to a session means the user is focused on that session
  (decided 2026-11-21). Matching the focused window to an agent process
  narrows the candidates; the most recent prompt picks among them.

### Server

- Verifies the chain and stamps its own receive time.
- Builds its own session UUID from the machine id, the agent and the agent's
  session id, so sessions from different machines or agents never collide.
  Each session on the server still has a name.
- Computes attention and the 0–1 multiplier per session.
- Removes overlap: if the multipliers of concurrent sessions for the same
  minute add up to more than 1.0, each session gets its share of one minute
  (two sessions at 1.0 each get 30 seconds each).
- Flags it when the agent's edit calls claim more changes than actually
  happened in the files, compared file by file.
- The client runs the same formulas only for display and to compare.
- Attention returns to 1.0 on input. It drops slowly while the agent's window
  stays focused and quickly in other apps. An agent working alone earns no
  credit; that time still counts as AI time.
- Quick switches don't farm attention. Switching to the agent's window for a
  few seconds (around 5) without real interaction gives little or no
  attention, because it could just be flicking between windows:
  - input (including the alt-tab key press) only counts in slots where the
    agent's terminal or editor held focus
  - watching without input only counts once the agent's window has held focus
    for about 20 seconds (two slots in a row); exact thresholds get tuned
    with real data

### Struggle

- Each heartbeat has a struggle value, calculated on both the client and the
  server.
- Struggle does not affect credit for now; it is only kept.
- Decided input: correction prompts (see prompt analysis).
- Possible future signals (not decided; ask before using them):
  - interrupts: the user stopped the agent mid-turn
  - rejected tool calls: the user denied an action the agent wanted to take
  - re-prompting: many near-identical prompts for the same task
  - reverts: Claude Code rewinds and opencode reverts
    The client already sees interrupts and rejected tool calls as events, and
    the data for re-prompting and reverts exists in the agents' files.

### Prompt analysis, on the device

- A really small local embedding model classifies prompts. Only the label
  leaves the machine.
- Correction prompts ("no, you did this wrong, redo it") add to struggle.
  They don't reduce time, for now.
- Pasted prompts earn less.
- Detecting AI-written prompts is low priority. Detectors are unreliable, so it
  must never decide anything on its own.

## Open questions

- What file changes the agent didn't make mean (for example, edits the user
  made), and how they count.

## For later

- Our own GNOME Shell extension for the focused app, so GNOME users don't
  need Window Calls.
- How `unknown` slots and heartbeats with `activity: false` count toward
  credit.
- Pasted error logs and stack traces are real struggle. They may need an
  exemption from the paste penalty.
- Which embedding model (small, multilingual). The architecture comes first.
- Attention decay speeds. Proposal: input sets attention to 1.0 for its slot,
  half-life 5 minutes while the agent's window is focused, 30 seconds in other
  apps, multiplier = the minute's average over its slots. Tune with real data.
- Reviewing after a turn ends (testing, reading the diff) earns nothing yet,
  because heartbeats stop when the agent stops.
- How struggle affects credit.
- Whether correction prompts should also reduce time.

## Ideas, not decided

- Typing rhythm: long pauses, deleting and rewriting a prompt. This needs a
  keyboard hook, which conflicts with "no keylogger".
- Detecting small pastes.
- Re-prompting and reverts. The data exists: Claude Code rewinds, opencode
  reverts.
- Prompt quality, and spotting prompts like "work for 10 hours and then
  return".
- Checking whether the AI did what was asked.
- Cross-checking AI usage with the provider.
- Measuring time spent looking at code, testing AI-generated code, coding
  by hand, and which files were worked on.
- Rewarding staying undistracted, and rewarding agent interruptions (the user
  watched and checked).
- Syncing with Hackatime or Lapse (raised in team chat; interop is optional
  and low priority).
- Collect lots of data first, then classify it. Evaluation should classify,
  not be exact.
- "Make the value of actions": the meaning is unclear; ask before acting on
  it.
- Against mouse jigglers and autotypers (raised 2026-11-21, nothing
  decided; ask first):
  - Input already only counts while the agent's window has focus, and
    attention needs agent-side evidence: prompts, tool approvals,
    interrupts, matching file edits. Moving the mouse produces none.
  - Synthetic input: macOS can compare hardware-only idle time
    (`HIDSystemState`) with the combined one, which also counts injected
    events. On Windows, injected input is only visible through low-level
    hooks (flag `LLMHF_INJECTED`), which read every key event. On X11, XTEST
    devices are distinguishable through XInput2. On Wayland, uinput tools
    (ydotool) look like hardware. Hardware USB jigglers can't be told apart
    anywhere.
  - Statistics on the server: perfectly regular input for hours, prompt
    cadence (always the same delay after a turn ends), no reading time
    proportional to the response length, typed characters far beyond what
    the input slots allow.
  - AI prompting AI: prompts that arrive while nobody touched the keyboard
    or the agent's window wasn't focused (tmux send-keys, pipes, the SDK)
    are visible as prompts without input. Claude Code records the
    entrypoint (CLI vs SDK) per session. An autotyper in front of the window
    can only be caught statistically or by the AI-prompt classifier (low
    priority, never decides alone).
  - Anything suspicious is flagged for reviewers, not decided
    automatically.
- Rejected: having the AI commit after every round. It conflicts with the
  rule against touching user repositories.
