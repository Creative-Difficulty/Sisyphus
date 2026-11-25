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
- Only counts, timings, coarse app classes, the operating system family,
  the client's version, the names of the models the agent used (decided
  2026-11-25), paths of edited files and of the project's own programs (relative to the project folder) and the
  agent's session titles leave the machine. Never
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
  - opencode: its SQLite database (v2 tables, or the v1 tables of older
    versions) and `prompt-history.jsonl`.
  - Codex: its rollout files (legacy and paginated formats, v0.32 and
    later) and `session_index.jsonl` for titles, followed live; only the
    user's own threads, not sub-agents, side chats or `codex exec` runs.
    Its patches count as edit claims, and its process is recognised in the
    focused window. Pasted characters can't be measured for Codex (it saves
    pastes expanded).
- One plugin per agent, behind the same interface.
- Events: prompt (with pasted characters), response, thinking, tool call and
  result (done / failed / rejected), interrupt, turn end, compaction, slash
  command, title change.
- Running detection: whether an agent is mid-turn.
- Pasted characters per prompt, from the prompt history. Agents only record
  large pastes, so small pastes count as typed.
- Activity on every OS, without a keylogger: time since the last input,
  key presses and mouse events counted (never which keys), and the focused
  app's class, including whether the focused window runs the session's
  agent (matched through the process tree).
  - Keys vs mouse: exact on macOS (CoreGraphics event counters); inferred
    on Windows and X11 (the last input time polled 25 times a second
    with the cursor and mouse buttons; input without mouse movement or a
    held button is a key, key events halved since releases count too); on
    Linux Wayland exact through the input helper (below), unknown without
    it.
- Heartbeat recording (v2 scheme), while the app runs (window open or in
  the tray): one heartbeat
  per session per minute, signed into the hash chain, queued on disk and
  uploaded every 30 seconds while logged in. File changes are found by
  polling the project folder while its agent runs; agent edit claims come
  from the edit tool calls. Each prompt is scored by the local prompt
  model (below).
  - Windows: `GetLastInputInfo`, the foreground window's process.
  - macOS: CoreGraphics idle time, `NSWorkspace`'s frontmost app.
  - Linux X11: XScreenSaver, `_NET_ACTIVE_WINDOW`.
  - Linux Wayland: `ext-idle-notify` (GNOME: Mutter's IdleMonitor); focus
    from Hyprland IPC, Sway IPC, the "Window Calls" GNOME Shell extension,
    a KWin script on KDE, or `wlr-foreign-toplevel-management`.
- The local prompt model (2026-11-22): scores every prompt from 0 to 1000
  for each of 22 categories (9 kinds of message, 13 topics), on the
  device, in any language; only the scores go into the heartbeat. A
  fine-tuned, compressed multilingual static embedding model
  (blink-multilingual-64M, MIT) with a small classifier, 26.7 MB, run in
  plain Rust. Downloaded from the Sisyphus server on first start and only
  used if its SHA-256 matches. Training tools and synthetic data:
  `tools/prompt-model/`.
- Client window (iced):
  - first start: log in, "Additional required files" (downloads the prompt
    model with a progress bar; required), connect agents
  - no consent step for activity (removed 2026-11-21); on Linux Wayland
    without the input helper, a popup that says exactly what setting it up
    asks for and installs it through polkit
  - settings to connect and disconnect agents
  - a debug screen (Ctrl+D twice): the current heartbeat's attention per
    session computed
    with the server's formula, what the tracker reads, uploads and the last
    heartbeat
  - a dashboard of recent sessions with today's credited time and agent
    time from the server (prompts and sessions are still placeholders)
  - runs in the tray (2026-11-24): closing the window keeps Sisyphus
    tracking in the tray; only "Quit" in the tray menu ends it. Hovering
    and the menu show today's tracked and agent time and the session an
    agent is working on (never the upload queue). Where there is no tray
    (plain GNOME) closing the window quits. On Windows and macOS a left
    click drops down a small panel at the icon with the same info big and
    "Open Sisyphus" (2026-11-24); the menu is on the right click
  - a placeholder logo (2026-11-24) as the window and tray icon, on the
    login step and on the website (favicon, header), until a real one
    exists
  - a pink light and dark theme that follows the OS
- Distribution (decided 2026-11-24):
  - download buttons on the website: "Download for <system>" next to
    "Log in with Hack Club" when logged out, a download icon in the header
    when logged in; both go through the server's `/download/<system>`,
    which redirects to the newest GitHub release's file
  - packages built by the release workflow: a Windows installer (per user,
    no admin rights, Start menu, starts at login by default, uninstaller),
    a macOS `.dmg` with one app for Apple silicon and Intel to drag into
    Applications (Dock icon only while the window is open), and Linux
    AppImages (x86_64, ARM)
  - silent over-the-air updates: the app checks the server every few
    hours, downloads the new version, checks it was signed by the release
    key (kept as a GitHub secret), and installs it when its window is
    closed and no agent is working, then starts again in the tray
  - not yet: code signing (Windows warns, macOS needs "Open Anyway" once),
    starting at login on macOS and Linux
- `sisyphus-common`: an ed25519-signed hash chain of heartbeats with
  tamper-detection tests.
- Server (axum + Postgres):
  - accepts signed heartbeat batches, checks the chain and signatures, and
    stores them with its own receive time
  - a web dashboard in the client's style with a sidebar: Overview, Projects
    (each project with its tracked time), Leaderboard and Settings (the
    time zone, the public profile)
  - public profiles (decided 2026-11-23): public by default, also for
    existing accounts, and turned off in the website's settings. A profile
    (`/u/ID`) shows the name, tracked and agent time for a date range, the
    time chart, time by language, task, agent and system, and each project
    with its time and its repository and live demo links. Never session
    titles or file paths. A private profile is visible only to its owner.
  - a leaderboard (decided 2026-11-23): public profiles ranked by tracked
    time, today, this week (default), the last 30 days or all time;
    clicking someone opens their profile
  - README badges (decided 2026-11-23): each project's popup has a badge
    maker (four styles, a preview, Markdown to copy). The badge shows the
    project's all-time tracked time and links to the profile; while the
    profile is private it says "not public"
  - the Overview's stats (decided 2026-11-23, modelled on Hack Club's
    Hackatime): a date range (today, this week, last 30 days, all time)
    and filters by project, agent, system, language and task; tracked
    time, agent time and counts; tracked time per hour/day/week; tracked
    time by project, language, task, agent and system, drawn as pink bars.
    - Languages come from the edited files' names: a minute's credit is
      split across the languages of the files that changed in it, by
      characters changed; minutes without file changes count as "No file
      changes".
    - Tasks come from the prompt model: each minute counts for the top
      task of its session's latest prompt ("No prompt yet" before one).
  - projects (decided 2026-11-23): clicking a project opens a popup with
    its stats (including prompts sent and its most changed files, by
    characters changed), its repository and live demo links, and fields
    to change them
  - prompts sent, on the Overview (for the date range and filters) and
    per project
  - git integration (decided 2026-11-23, like Hack Club's Hackatime):
    users connect GitHub on the website (OAuth, public data only; the
    token is kept so the server can search as them, and deleted on
    disconnect). The server then links each project to the public
    repository with the project's folder name, in the user's account, in
    one of their organisations where they have commits, or among the
    public repositories they contributed to (commits or pull requests),
    only if exactly one matches. Nothing new is sent from the client: git remotes are not
    uploaded. Private repositories and renamed folders are set by hand in
    the project's popup, which also takes a live demo link. Links are
    visible only to the user (and reviewers).
  - a debug view, Heartbeats (dotted outline in the sidebar), listing every
    heartbeat the user's devices uploaded; clicking one shows the stored
    record in a popup
  - a Devices view (decided 2026-11-22): each linked device with its
    operating system's icon and tracked time, and removing it. Removing
    unlinks it: it can't upload until it logs in again, and its heartbeats
    and tracked time stay in the account. Each stored heartbeat remembers
    the account it was uploaded to.
  - a Dockerfile for deployment
  - accounts via Hack Club Auth (OpenID Connect) for the website and the
    app: the server is the only OAuth client; the app logs in through it and
    links its device key to the account; the browser asks "Log in the
    Sisyphus app on this computer as <name>?" before a device is linked
    (decided 2026-11-22)
  - uploads accepted only from devices linked to an account; each user sees
    only their own devices and heartbeats
  - signed app requests: today's credited and agent time, and logging the
    device out on the server
  - crediting: server-side sessions (UUID from device, agent and agent
    session id), attention per slot, the multiplier per minute, sharing
    concurrent sessions' minutes, AI time kept apart, offline flags; rebuilt
    from the stored heartbeats when the formula changes
  - against minting time (2026-11-23): heartbeats must start on a whole
    minute; heartbeats dated before their device was linked are stored but
    earn nothing (late ones earn credit with a flag, 2026-11-25); crediting locks
    per user, so two devices can't both keep a full share of one minute
  - the review page Pulse and its flags (2026-11-23) were removed
    completely on 2026-11-25
  - Not built: the struggle value, applying the edit-claim check to
    credit, the mouse-jiggle rule.

## Decided, not built

### Heartbeat scheme (final, approved 2026-11-20)

- Time is counted and calculated per agent session. Each session sends its own
  heartbeat every minute while its agent runs, or while it doesn't run but
  its agent's window is in front (decided 2026-11-21; mouse-jiggle handling
  comes later).
- For now the client records only while the app runs (its window open,
  or closed to the tray since 2026-11-24); a background service comes
  later (decided 2026-11-21).
- `working_dir` changes are found by checking the project folder for
  changes while the agent runs (decided 2026-11-21).
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
    "version": 12,
    "minute_ms": 1795201380000,
    "os": "linux",

    "agent": "claude-code",
    "agent_session_id": "77ae4435-adc7-40d4-934f-f70f97ced5e4",
    "session_name": "Client traits for agent session events",
    "project": "Sisyphus",

    "agent_running": true,
    "input_kinds": "exact",
    "slots": [
      { "input": true, "keys": 41, "mouse": 0, "focus": "agent" },
      { "input": true, "keys": 12, "mouse": 30, "focus": "agent" },
      { "input": false, "keys": 0, "mouse": 0, "focus": "agent" },
      { "input": false, "keys": 0, "mouse": 0, "focus": "browser" },
      { "input": true, "keys": 0, "mouse": 212, "focus": "editor" },
      { "input": false, "keys": 0, "mouse": 0, "focus": "testing" }
    ],

    "prompts": {
      "typed_chars": 86,
      "pasted_chars": 0,
      "model": "sisyphus-prompt-1",
      "classes": [
        [3, 912, 40, 0, 2, 31, 0, 0, 1, 12, 770, 0, 4, 0, 0, 96, 0, 2, 0, 0, 1, 3]
      ],
      "at": [42],
      "scripted": 0
    },

    "interrupts": 0,

    "responses": {
      "at": [{ "second": 12, "chars": 4200 }]
    },

    "files": [
      { "path": "CLAUDE.md", "working_dir": { "added": 200, "removed": 7 } }
    ],

    "injected": { "keys": 0, "mouse": 0 },

    "client": "0.3.0",
    "models": ["claude-opus-5-5"],
    "programs": [{ "path": "target/debug/game", "in_front": true }],
    "tool_calls": [
      { "kind": "read", "at": 8, "ms": 310, "outcome": "ok" },
      { "kind": "edit", "at": 14, "ms": 2100, "outcome": "ok",
        "files": [{ "path": "CLAUDE.md", "added": 193, "removed": 7 }] },
      { "kind": "shell", "at": 20, "ms": 8400, "outcome": "rejected" }
    ]
  }
}
```

- Version 12 (decided 2026-11-25): nothing that can be computed from
  another field is sent. Gone: `activity` (always read), the prompt count
  (`prompts.at` has one entry per prompt), `prompts.corrections` (from the
  scores), the response count and characters (from `responses.at`).
  `tested_programs` and `running_programs` became `programs`, each with
  `in_front`. Each tool call is recorded once in `tool_calls`, in the minute
  it finished: its kind, the second, how long from call to result, its
  outcome (`ok`, `error`, `rejected`, or `unfinished` when the turn ended
  first) and, for successful edits, the characters it claims per file.
  This replaces `tools` and the files' `agent_edits`; `files` lists only
  what changed in the working directory. The server still accepts v9 to
  v11.

- Version 11 (decided 2026-11-25): no more `seconds`. Keys and mouse are
  counted only per 10-second slot, as before v9; 120 numbers a minute
  were too much.

- Version 10 (decided 2026-11-25, from comparing with sablier):
  - `client`: the Sisyphus app's version that recorded the heartbeat, so
    reviewers can tell a known client bug from a suspicious pattern.
  - `models`: the models that worked in the session this minute, as the
    agent names them (`claude-opus-5-5`), sorted.
  - `tools`: the agent's tool calls by kind (`shell`, `edit`, `read`,
    `web`, `agent` for sub-agents, `other`): calls started this minute and
    the milliseconds from call to result of those that finished this
    minute (including the time the user took to approve). Never a tool's
    input or output. Shows how much the agent did on its own.
  - The server still accepts v9 from apps that haven't updated yet.

- Version 9 (decided 2026-11-23, against autotypers and jigglers):
  - `seconds` (dropped again in v11): key presses and mouse events in each of the minute's 60
    seconds, for the whole system (never which keys); empty without
    activity or when keys and mouse can't be told apart. People type in
    bursts; machines keep time.
  - `injected`: the part of the minute's input that came from software,
    where the system tells (macOS events posted by apps, Linux virtual
    devices through the input helper); 0 elsewhere. Key remappers count
    too, so it's never shown publicly.
  - `prompts.at` / `responses.at`: the second each prompt was sent and
    each response finished (with its length), so reply delays can be
    compared with reading time.
  - `prompts.scripted`: prompts the agent says a program sent (Claude
    Code's SDK, `claude -p`).

- `minute_ms`: the minute the client claims; the server checks it against its
  receive time.
- `agent`, `agent_session_id`: the server builds its session UUID from device
  id + agent + agent session id.
- `session_name`: the agent's own session title. Visible only to the user and
  reviewers.
- `project`: the working directory's folder name only.
- `os` (decided 2026-11-22, scheme version 7): the client's operating
  system family, always sent: `windows`, `macos`, `linux` or `other`.
  Never a version, distribution or machine name. The website's Devices
  view shows it as an icon.
- `activity` (added 2026-11-21): whether activity was read. Since the
  consent step was removed (2026-11-21) it is always `true`; `false` sends
  every slot as `{ "input": false, "keys": 0, "mouse": 0, "focus":
  "unknown" }`.
- `input_kinds` (decided 2026-11-21, scheme version 5): how key presses
  and mouse events were told apart this minute: `exact` (macOS, Linux
  with the input helper), `inferred` (Windows, X11) or `unknown` (Linux
  Wayland without the helper; `keys` and `mouse` are then 0). The weakest
  way seen in the minute.
- `slots`: the minute's six 10-second slots.
  - `input` is any keyboard or mouse input, read from the OS idle time; keys
    are never recorded.
  - `keys`: key presses in the slot (never which keys). `mouse`: mouse
    events (movements, clicks, scrolling, touchpad touches).
  - `focus` is one of `agent` (the agent's own window), `terminal`, `editor`,
    `browser`, `other`, `testing` or `none`: the app that held focus for most of the
    slot, so a short flash into another window doesn't change it. `unknown`
    when activity isn't allowed, or the system can't tell (for example GNOME
    without the Window Calls extension).
- `testing` and `tested_programs` (decided 2026-11-21, scheme version 4;
  since v12 `programs` entries with `in_front`):
  the user is testing what they build. A slot's focus is `testing` when the
  window in front runs an executable that lives inside the session's
  project folder, and that folder is a git repository (it has `.git`).
  Programs an agent started (its tests, its tools) don't count, and the
  agent's own window stays `agent`. `tested_programs` lists those
  executables' paths relative to the project folder, sorted, never
  absolute. Scripts run by an interpreter outside the project (`python
  main.py`, `node server.js`) aren't recognised, because the executable is
  the interpreter.
- `running_programs` (decided 2026-11-21, scheme version 6; since v12
  `programs`): the
  project's own programs (same rule as `testing`, any window or none)
  that ran this minute, relative paths. While one runs, the project's
  most recently active session sends a heartbeat every minute even when
  its agent is idle and its window isn't in front. The slots where the
  program is in front are `testing`. It earns no attention or credit for
  now (decided 2026-11-21). The tracking process itself counts too
  (decided 2026-11-21), so a Sisyphus built from its repo counts as
  running for that repo.
- `interrupts` (added 2026-11-21, scheme version 3): times the user
  stopped the agent mid-turn this minute.
- `prompts`: counts and the local model's scores only, never text.
  - Corrections are prompts the local model scored at 500 or more as
    corrections (computed from `classes` since v12).
  - `model` (decided 2026-11-22, scheme version 8): the prompt model's
    name, empty when no prompt was scored this minute.
  - `classes` (decided 2026-11-22, scheme version 8): per scored prompt, in
    the order they were sent, the probability of each of the model's 22
    categories as a whole number from 0 to 1000 (heartbeats have no
    floats). Fewer entries than prompts if the model wasn't loaded.
- `responses` (added 2026-11-21): never text. Per response (`at`) the
  second it finished and the characters of its visible text (not
  thinking, not tool calls). Lets the server check reading time: a human
  can't sensibly answer a long response seconds after it ends.
- `files`: one entry per file that changed in the working directory this
  minute, sorted by path.
  - `path` is relative to the project folder.
  - `working_dir` counts non-whitespace characters added and removed, as
    actually seen in the file while the agent runs.
  - What the agent's edit calls claim is in `tool_calls` (v12; before, the
    file's `agent_edits`).
- Input is detected without a keylogger hook: OS "time since last input"
  APIs, key/mouse counts (see Built), plus the class of the focused app.
- Activity is read without asking (decided 2026-11-21: the consent step
  and the settings row are gone on every system).
- The input helper (decided 2026-11-21), only on systems that can't tell
  keys from mouse otherwise (Linux Wayland): a popup at start explains
  exactly what is requested (what it counts, what gets installed, that it
  doesn't run as root, how to remove it, and to contact @simon on Slack
  with concerns), then polkit (`pkexec`) asks for the password once. It
  installs a copy of the app at `/usr/local/libexec/sisyphus-input-helper`
  and the systemd service `sisyphus-input-helper`, which runs as a
  throwaway user (`DynamicUser`) in the `input` group, without network or
  capabilities, reads `/dev/input` and sends only key and mouse counts to
  the installing user over `/run/sisyphus-input/socket`. The popup can't
  be skipped (decided 2026-11-21: the counts are needed); the app only
  continues once the helper runs. Settings has an "Input helper" row
  (Linux Wayland, or wherever it's installed): Running / Not running,
  Restart, Uninstall, or Set up when it doesn't answer; each asks for the
  password through polkit (decided 2026-11-21).
- GNOME doesn't expose the focused window. For now it is read through the
  third-party "Window Calls" extension when installed, otherwise unknown.
  Shipping our own GNOME Shell extension is for later.
- The agent's edit calls and the working directory's changes must match up,
  file by file.
- A prompt submitted to a session means the user is focused on that session
  (decided 2026-11-21). Matching the focused window to an agent process
  narrows the candidates; the most recent prompt picks among them. Claude
  Code names the session of each process, which makes the match exact. Its
  session manager (`claude agents`) doesn't say which background session it
  shows, so there the background session with the latest prompt counts as
  in front (decided 2026-11-21).

### Server

- Verifies the chain and stamps its own receive time.
- Refuses heartbeats dated in the future (decided 2026-11-21): a
  heartbeat whose minute hasn't ended yet by the server's clock can't have
  been recorded, so the batch is refused. The app keeps those heartbeats
  and sends them once their minute has passed on the server.
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
- The app gets the day's total time from a server endpoint; it doesn't
  compute it itself (decided 2026-11-21).
- Only a valid, logged-in app may send heartbeats or read data (decided
  2026-11-21). The app signs every request with its device key (signed
  requests, no token); the server checks the signature, the time, replays
  and that the device is linked to an account. Logging out in the app
  unlinks the device on the server.
- Attention returns to 1.0 on input. It drops slowly while the agent's window
  stays focused and quickly in other apps. An agent working alone earns no
  credit; that time still counts as AI time.
- Attention is one value per heartbeat (decided 2026-11-21, formula 3):
  interaction while the agent's window is in front makes the whole
  heartbeat 1.0, so writing and sending a prompt guarantees 1.0 for that
  minute.
- Interaction in the agent's window (decided 2026-11-21, formula 4): more
  than 5 key presses in the heartbeat's slots where it was in front; mouse
  events don't count there. In all other apps, including testing the
  project's programs, keys and mouse both count as activity (the slot's
  `input`). Where keys and mouse can't be told apart (`input_kinds`
  unknown), any input in the agent's window still counts.
  Otherwise it decays from the session's previous heartbeat. The slots are
  only the readings it's computed from.
- Decay speeds (decided 2026-11-21): half-life 2 minutes while the agent's
  window stays in front (for most of the minute), 30 seconds otherwise
  (changed from 20 s on 2026-11-21). The multiplier is the heartbeat's
  attention.
- With activity off (decided 2026-11-21), only agent events give attention:
  a prompt is worth 1.0 and an interrupt 0.5, falling to 0.0 at the next
  heartbeat. Other user actions may get some low values later (which ones
  and how much isn't decided).
- Implementation choices to confirm: a minute where no slot's focus is
  known counts like activity off; a single `unknown` slot counts like
  being in another app.
- `testing` earns no attention for now (decided 2026-11-21): it counts
  like any other app (attention decays as when leaving). Minutes where
  the project's program runs are sent (`running_programs`).
- No time after a turn ends for now (decided 2026-11-21): only minutes in
  which the agent ran earn credit. Attention is still followed through the
  others.
- Offline heartbeats count (decided 2026-11-21); the server flags them
  (`offline`, more than 5 minutes late) on the stored heartbeat. However
  late they arrive, they earn credit (decided 2026-11-25; the one-hour
  limit of 2026-11-23 is gone). A heartbeat dated before its device was
  linked to the account (recorded while logged out) is stored but earns
  nothing.
- Heartbeats must start on a whole minute (decided 2026-11-23), and
  crediting is serialised per user, so one real minute can never be
  credited more than once in total.
- The review page Pulse, its flags and the `REVIEWERS` setting (decided
  2026-11-23) were removed completely (decided 2026-11-25).
- No mouse-jiggle rule for now (decided 2026-11-21); it comes later and has
  to exist before credit can be trusted.
- The user sets their time zone in the website's settings; "today" and
  "this week" (from Monday) follow it, UTC until set (decided 2026-11-21).
  The app's total uses it too, or the app's own midnight while none is
  set.
- Quick switches don't farm attention. Switching to the agent's window for a
  few seconds without real interaction gives nothing, because it could just
  be flicking between windows:
  - input (including the alt-tab key press) only counts in slots where the
    agent's terminal or editor held focus, and there only more than 5 key
    presses (formula 4)
  - attention never rises without such input; watching without input only
    slows the decay, and only when the agent's window held focus for most
    of the minute (replaces the earlier two-slots rule, 2026-11-21)

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

- A small local model classifies prompts. Only its scores leave the
  machine.
- Many labels that cover all kinds of prompts, not only corrections
  (decided 2026-11-22). For now the draft set is used (2026-11-22): 9 acts
  and 13 tasks, defined in `tools/prompt-model/labels.md`. Changing them
  means a new model.
- The model gives each category a probability from 0.0 to 1.0, and those
  probabilities are sent (decided 2026-11-22), as whole numbers from 0 to
  1000.
- The model (2026-11-22): blink-multilingual-64M (MIT) fine-tuned together
  with a 2-layer classifier, compressed to 26.7 MB. Fine-tuning may only
  change word pieces that occur in the public, LLM-written training data,
  so the published model doesn't reveal words from the private real
  prompts it was also trained on.
- It isn't bundled with the app: the first start downloads it from the
  Sisyphus server on a screen labelled "Additional required files", which
  can't be skipped (decided 2026-11-22). The app checks the file's SHA-256.
- The model should be about 20 MB; a little over is fine, far over (like
  70 MB) isn't (decided 2026-11-22).
- It works with prompts in any language (decided 2026-11-22).
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
- Pasted error logs and stack traces are real struggle. They may need an
  exemption from the paste penalty.
- Improving the prompt model: conversation context (did the agent just ask
  something or change something) for corrections and clarifications, real
  prompts from more people, splitting the catch-all `other` task.
- Tuning the decay speeds with real data.
- The mouse-jiggle rule. Proposal: minutes where the agent's window is in
  front but the agent doesn't run only count up to a budget before the next
  prompt (reading time from the previous response's length, typing time from
  the prompt's length, a capped thinking allowance).
- Reviewing after a turn ends (testing, reading the diff) earns nothing
  (decided 2026-11-21: no time after a turn ends for now).
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
- Against mouse jigglers and autotypers (raised 2026-11-21). Built on
  2026-11-23 at the user's request: per-second input in heartbeats (v9;
  dropped again in v11, 2026-11-25),
  injected input where the OS tells (macOS posted events, Linux virtual
  devices), scripted prompts (Claude Code's SDK / `claude -p`), prompts
  only marking their slot as the agent's from a terminal, editor or
  unknown window, agents in tmux matched to the terminal showing them,
  and the reviewer flags above. Still ideas (ask first):
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
- From sablier (espcaa/sablier, another AGPLv3 entry for the bounty;
  raised 2026-11-21, nothing decided; ask first):
  - Approvals as a signal: approving a tool call is human work, and the
    time the permission dialog was open is real attention. Today only
    rejected tool calls are events; approval latency might be inferred from
    the transcript's tool call and result timestamps.
  - Credit backwards from a prompt: the time between the agent's last turn
    end and the next prompt was spent reading and writing, up to a cap.
    A simple starting point for the reading/typing budget in the
    mouse-jiggle proposal.
  - "Approval-like" prompts: flag prompts that are only "yes", "continue",
    "ok", "lgtm" and similar (a fixed list, matched on the device; only the
    flag leaves the machine). Cheap, needs no embedding model, and catches
    farming by spamming "continue".
  - A salted prompt hash (HMAC of the normalized text, salt stays on the
    device) to find exactly repeated prompts for the re-prompting signal,
    without uploading text. It doesn't catch near-identical prompts.
  - Project identity from the git `origin` remote (or a project file like
    `.wakatime-project`), so the same repo matches across machines and
    folder renames. Would also help the git integration.
- Gaps compared to steer (EwoudVV/steer, another AGPLv3 entry for the
  bounty, by duck, live at steer.duck.hackclub.app; raised 2026-11-23,
  nothing decided; ask first). steer infers time from the gaps between
  prompts and doesn't read input or focus, but it is ahead on:
  - Public profiles (bounty requirement 5): built on 2026-11-23 (profiles,
    leaderboard, badges). steer also has a public page per project.
  - Submitting a project: one command freezes a project's hours into a
    review page that never changes, with a written justification, the
    checks, every day and session, and the commits checked against GitHub
    (missing or differently timed commits marked). Reviewers log in with
    Hack Club to approve, cut or reject the hours with a note the user
    sees, and every event behind the page can be downloaded. We have no
    review page (Pulse was removed on 2026-11-25).
  - Hosting cost (bounty requirement 11): a load test that runs the real
    server with simulated clients, and a written estimate of the cost at
    the bounty's numbers.
  - Install and updates: a one-line installer (`curl … | sh`, `irm … |
    iex`), a `doctor` command that checks the setup, an `uninstall`, and
    automatic updates checked against GitHub releases once a day.
    (Installers, a `.dmg`, AppImages and signed automatic updates were
    built on 2026-11-24.)
  - More agents (bounty requirement 7): Copilot (VS Code and CLI), Cursor
    and Freebuff, and ChatGPT and claude.ai in the browser through a
    Chrome extension.
  - A public "faking time" page for reviewers: what is trusted, what is
    checked, what isn't caught, and the results of AI agents trying to
    fake time.
  - Checking the counting: a simulation of different kinds of users
    compared counting rules against real logs, and optional check-ins
    ("what are you doing right now?") collect ground truth to tune them.
  - Coding by hand between prompts counts: file changes and commits within
    30 minutes of using an AI tool.
- Rejected: having the AI commit after every round. It conflicts with the
  rule against touching user repositories.
