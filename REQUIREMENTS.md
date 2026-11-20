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
- Only counts, timings and coarse app classes leave the machine. Never prompt
  text, file contents, file paths, window titles or keystrokes.
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
- Client window (iced):
  - settings to connect and disconnect agents
  - a dashboard of recent sessions with placeholder "Recorded today" figures
  - a pink light and dark theme that follows the OS
- `sisyphus-common`: an ed25519-signed hash chain of heartbeats with
  tamper-detection tests.

## Decided, not built

### Heartbeats

- Time is counted and calculated per agent session. Each session sends its own
  heartbeat every minute while its agent runs or its prompt is being written,
  with a unique session id.
- Heartbeats carry integer observations only, never a 0–1 value from the
  client:
  - which of the minute's six 10-second slots had input while the agent's
    terminal or editor was focused
  - seconds focused on the agent's app vs other apps, by app class only
  - whether the agent was running
  - prompt characters typed and pasted
  - code changes, as two sets of counts of non-whitespace characters added and
    removed (never paths or contents):
    - file changes: what actually changed in the agent's working directory
    - agent changes: what the agent's edit tool calls say they changed
  - prompt classification labels
- Input is detected without a keylogger and without root:
  - OS "time since last input" APIs
  - the class of the focused app
- On Wayland, reading the focused app needs the user's permission: a popup
  asks for it and explains how the data is used.
- While an agent runs, the client watches its working directory. The agent's
  edit calls and the changes in the files must match up.
- Heartbeats are signed into the hash chain, queued offline and uploaded about
  every 30 seconds.

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
  happened in the files.
- The client runs the same formulas only for display and to compare.
- Attention returns to 1.0 on input. It drops slowly while the agent's window
  stays focused and quickly in other apps. An agent working alone earns no
  credit; that time still counts as AI time.

### Struggle

- Each heartbeat has a struggle value, calculated on both the client and the
  server.
- Struggle does not affect credit for now; it is only kept.

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
- Session names: agent titles are generated from prompts, so sending them may
  conflict with "prompt text never leaves the machine". Send the title, let
  the user rename it, or name sessions from project and date?
- How the focused app is read on Wayland once the user agrees (per
  compositor).

## For later

- Pasted error logs and stack traces are real struggle. They may need an
  exemption from the paste penalty.
- Which embedding model (small, multilingual). The architecture comes first.
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
- Rejected: having the AI commit after every round. It conflicts with the
  rule against touching user repositories.
