# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

All requirements, decisions and ideas: @REQUIREMENTS.md

## Project

Sisyphus is an open-source (AGPLv3) passive time tracker in Rust. It measures the hours of "productive struggle" that teenagers spend building projects with AI. It is the team's entry for the Hack Club AI time tracking bounty. The server is the source of truth: reviewers read what the server stored, never the client.

The team talks in German; code, comments, docs and commit messages are in English. Always write "Hack Club" exactly like that.

## Rules

- Only implement what the user explicitly asked for in the current conversation. Requirements, plans and ideas are context, not tasks. If a request is ambiguous, ask.
- Don't decide open questions yourself (see "Open questions" in `REQUIREMENTS.md`). Ask.
- When the user makes or changes a decision, record it in `REQUIREMENTS.md`, and in this file if agents need it to work.
- Privacy:
  - Never collect or upload file contents, absolute paths, window titles, keystroke content or prompt text.
  - Only counts, timings, coarse app classes, the operating system family, the local prompt model's per-category scores, the client's version, the names of the models the agent used, paths of edited files and of the project's own programs relative to the project folder, and the agent's session titles may leave the machine. Paths and titles are visible only to the user and reviewers, never on public profiles.
  - AI chat history and prompts stay local. `AgentEvent`s carry full prompt and response text in memory on the client; anything that leaves the machine must be reduced to what the heartbeat scheme allows.
- Never write to, commit to or modify a user's repository. Everything runs in the background.
- Never credit more time than was actually tracked. Multipliers from 0.0 to 1.0 are applied server-side.
- Never trust the client clock for anything audited. The server timestamps.
- No new dependencies without a one-line justification and the user's approval. Put new ones in `[workspace.dependencies]`.
- Keep Windows, macOS and Linux working, and keep the client light (low-spec machines).
- Commit and push finished work automatically, without asking (`git push origin main`). Before pushing, `git fetch` and rebase if someone else pushed. Keep commits separate by topic. Pushing `main` deploys the server to production, so only push tested server changes.
- Before committing: `cargo build`, `cargo test --workspace` and `cargo clippy` clean, `cargo fmt` applied to the crates you touched.
- Never build, pull or run Docker/Podman images. The `Dockerfile` is for deployment only; the user dislikes Docker locally.

## Commands

```sh
cargo build                                  # whole workspace
cargo build --release -p sisyphus-client     # release client
cargo build --release -p sisyphus-client --no-default-features  # as the release workflow builds it (no debug screen, no CLI)
cargo test --workspace                       # all tests
cargo test -p sisyphus-common --test chain   # one integration test file
cargo test -p sisyphus-client opencode::session::tests::abort_is_an_interrupt  # one test, by path filter
cargo clippy --workspace --all-targets
cargo fmt -p sisyphus-client                 # crates/common is not rustfmt-clean yet; don't reformat it unasked

cargo run -p sisyphus-client                                  # open the window
cargo run -p sisyphus-client -- sessions                      # list sessions of all agents, with running state
cargo run -p sisyphus-client -- watch [SESSION_ID] [--replay] # print a session's events live; never exits on its own
cargo run -p sisyphus-client -- activity                      # print idle time, focused app and per-session agent focus each second; never exits
cargo run -p sisyphus-client -- track                         # record and upload heartbeats without the window, printing each; never exits

cargo run -p sisyphus-server                 # needs DATABASE_URL (from the environment or .env, see .env.example)
```

No `rust-toolchain` file. CI: `.github/workflows/release.yml` runs when a `v*` tag is pushed (`git tag v0.2.0 && git push origin v0.2.0`; the tag must match `[workspace.package] version`, so bump that first). It builds the client without the debug screen and the CLI (`--no-default-features`), packages it (`packaging/`, decided 2026-11-24) and publishes a GitHub release: `Sisyphus-windows-x86_64-setup.exe` (Inno Setup, `packaging/windows/sisyphus.iss`), `Sisyphus-macos-universal.dmg` (one `lipo` binary for Apple silicon and Intel in `Sisyphus.app`, ad-hoc signed, with an Applications shortcut to drag onto; `packaging/macos/app.sh`) plus the same app as `.app.tar.gz` for updates, `Sisyphus-linux-{x86_64,aarch64}.AppImage` (`packaging/linux/appimage.sh`, pinned appimagetool and static type2 runtime, so no libfuse2) plus the bare binary as `sisyphus-linux-<arch>.tar.gz`, `update.json` + `update.json.sig` (see "Updates" below) and `SHA256SUMS.txt`. Keep the file names: the server's download route and the updater find files by them. Run by hand (`gh workflow run release.yml`) it's a dry run that keeps everything as the run's artifacts and publishes nothing. It doesn't run tests or build the server. No code signing yet: Windows SmartScreen warns, and macOS needs "Open Anyway" in Privacy & Security the first time. Edition 2024, resolver 3 (let-chains are used). Dependencies are declared in `[workspace.dependencies]` in the root `Cargo.toml` and referenced with `.workspace = true`.

## Architecture

Three crates in `crates/`:

- **`sisyphus-common`**: types shared by client and server.
  - Data flow (see `lib.rs` docs): collectors produce `Heartbeat`s → the client signs them into a per-device hash chain and queues them offline → batches are uploaded (`api.rs`) → the server verifies the batch continues the stored `ChainTip`, stamps its own receive time, and flags heartbeats received more than 5 min after their claimed time as offline.
  - `chain.rs`: sequence number, previous hash, SHA-256 with domain tag `sisyphus/heartbeat/v1`, ed25519 signature.
  - Heartbeats are hashed and signed, so they must serialize deterministically: no floats, no maps. Changing the hashed format means bumping the domain tag.
  - The tamper-detection tests in `crates/common/tests/` must keep passing.
  - `heartbeat.rs` is the scheme from `REQUIREMENTS.md` ("Heartbeat scheme"), version 11 (v3 added `interrupts`, v4 the `testing` focus and `tested_programs`, v5 per-slot `keys`/`mouse` and `input_kinds`, v6 `running_programs`, v7 `os`, v8 `prompts.model` and `prompts.classes`, v9 per-second `seconds.keys`/`seconds.mouse`, `injected`, `prompts.at`/`prompts.scripted` and `responses.at`, v10 `client`, `models` and `tools`, v11 dropped `seconds` again (decided 2026-11-25: the slots' key and mouse counts are enough; `Heartbeat::seconds` is an `Option`, present only in v9/v10 heartbeats, so they still hash as before)): one heartbeat per agent session per minute, domain tag `sisyphus/heartbeat/v<version>`. `verify_batch` accepts `MIN_VERSION` (9) to `VERSION` (11) and rejects others (`UploadError::UnsupportedVersion`), so released clients keep uploading until they update (decided 2026-11-25); v10's fields are left out of the encoding when empty, so v9 heartbeats hash as before (`v9_heartbeats_hash_as_before`). The client's queue drops versions the server no longer accepts on load and re-aligns the chain.
- **`sisyphus-client`**: a library (`lib.rs`: `activity`, `agents`, `config`, `device`, `gui`, `login`, `logo`, `prompt_model`, `tracker`, `tray`) plus a binary (`main.rs`).
  - `prompt_model/`: the local prompt classifier, see "Prompt model" below.
  - Run without arguments, the binary opens the iced window; `--background` (in every build; the installer's login entry and updates use it) starts in the tray without the window, once first start is done and there is a tray; `sessions`, `watch`, `activity` and `track` are CLI commands (`cli.rs`), only with the `cli` feature (default on). Release builds from CI leave it out (decided 2026-11-23): a window-only app that opens the window whatever the arguments, and on Windows a GUI-subsystem `.exe` without a console window. `input-helper` (Linux) stays in every build, because polkit and the helper's service run the app with it.
  - `device.rs`: this install's ed25519 key, created once from OS randomness and stored as `device.key` (hex) next to the config, mode 0600 on Unix. It never leaves the machine; it signs heartbeats and proves the device when linking.
  - `login.rs`: logs the app in through the server (see "Accounts" below). Blocking; the window runs it on a worker thread. It refuses non-https servers except on localhost.
  - `server.rs`: signed requests to the server (see "Signed app requests" below): `today()` and `logout()`, plus the local day's start (`libc::localtime_r` on Unix, `GetTimeZoneInformation` on Windows).
  - `activity/`: time since the last input and the focused app, per OS (see "Activity" below).
  - `tracker/`: builds, signs, queues and uploads heartbeats (see "Tracker" below).
  - `update.rs`: over-the-air updates (see "Updates" below).
  - `build.rs`: on Windows targets, embeds `assets/logo.ico` and version info in the `.exe` (`winresource`, build-only).
- **`sisyphus-server`**: axum + Postgres (sqlx).
  - Configured from the environment or `.env` (`.env` is git-ignored; `.env.example` lists every key): `DATABASE_URL`, `BIND_ADDR` (default `0.0.0.0:8080`), `PUBLIC_URL` (login redirect and whether cookies are `Secure`), `HACKCLUB_CLIENT_ID` / `HACKCLUB_CLIENT_SECRET` (login is disabled without them), optional `HACKCLUB_AUTH_URL`, optional `GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET` (connecting GitHub, see "Projects and GitHub" below; `GITHUB_URL` / `GITHUB_API_URL` point it at a fake GitHub for tests).
  - Migrations in `crates/server/migrations/` are embedded (`sqlx::migrate!`) and run on start.
    - `devices`: device key, chain tip, first/last seen, owning user, `linked_ms` (when it was last linked; 0 for devices linked before the column existed).
    - `heartbeats`: device, seq, hash, prev, sig, claimed and received time, and the heartbeat body as `JSONB` (so the v2 format won't need a table rewrite), and `user_id`: the account the device was linked to when it was uploaded. Heartbeat queries, the Heartbeats view and `backfill` go by this, never by `devices.user_id`, so unlinking a device keeps its history and time with that account.
    - `users` (with `timezone`, `public_profile` (default true, see "Public profiles" below) and `github_id`/`github_login`/`github_token`), `sessions`, `login_attempts`, `app_codes`: see "Accounts" below.
    - `projects`: per user and project folder name, `repo_url` (found on GitHub or set by the user; `repo_manual` once the user set or cleared it), `repo_checked_ms` and `repo_checked_by` (last automatic lookup and the matcher version that made it) and `demo_url`. See "Projects and GitHub" below.
    - `heartbeats.offline`: received more than 5 min after its minute (set on upload).
    - `heartbeats.stale` (decided 2026-11-23): received more than an hour after its minute (`api::STALE_AFTER_MS`; agents need the network, so real work isn't recorded offline for long), or dated before its device's `linked_ms` (recorded while logged out). Stored so the chain stays whole, credited 0.
    - `agent_sessions`, `session_minutes`, `minute_languages` and `minute_files`: crediting and the stats, see "Crediting" below.
  - Queries are runtime-checked (`sqlx::query`, not the `query!` macros), so building needs no database.
  - `POST /api/upload` (`upload.rs`): an `UploadBatch` from `sisyphus-common::api`.
    - Only devices linked to an account may upload (403 otherwise).
    - The device's row is locked and its chain tip checked with `verify_batch`.
    - On success the heartbeats are stored with the server's receive time and offline flag, credited in the same transaction, and an `UploadAck` with the new tip comes back.
    - Heartbeats dated in the future are refused (decided 2026-11-21): if any heartbeat's minute hasn't ended by the server's clock (`api::is_future_dated`: `minute_ms + 60 s > now`), the whole batch gets 400 `FutureDated { at }`. The client (`queue.rs`) then sends only the heartbeats before `at` and keeps the rest queued until their minute has passed on the server (the debug screen says the computer's clock is ahead).
    - Heartbeats must start on a whole minute (`minute_ms % 60000 == 0`, decided 2026-11-23); otherwise the batch gets 400 `Misaligned { at }`, since sharing a minute among sessions only works on whole minutes (one real minute claimed at T, T+1 s, … would each earn a minute). The client always sends whole minutes.
    - Crediting takes a per-user lock (`pg_advisory_xact_lock(user_id)`, after the device row lock), so two of a user's devices uploading the same minute at once can't both keep a full share.
    - 409 with `OutOfSync` (resend from the expected tip) or 400 with `Empty`/`Invalid`/`UnsupportedVersion`/`FutureDated`/`Misaligned`.
  - `GET /api/dashboard?range=…&project=…&agent=…&os=…&language=…&task=…` (`stats.rs`, decided 2026-11-23): the Overview's stats. `range` is `today` (default), `week` (from Monday), `30d` (with today) or `all`, in the user's time zone (UTC until set); each filter is optional, and an empty value selects minutes without one (unknown system, no file changes, no scored prompt yet). Returns tracked time, agent time, session, project and prompt counts, `buckets` (one per hour for today, per day, or per week for all time beyond 92 days; empty ones included, labelled in `timezone`), tracked time `by_project`/`by_agent`/`by_os`/`by_language`/`by_task`, `files` (only with a project filter, since paths are relative to each project: its 10 most changed files by characters added and removed, only the filtered language's files with a language filter), and `options` (every value each filter can take, all time). All SQL, with fixed parameters; the per-column queries are built only from constants (`AssertSqlSafe`).
    - Tracked time is the sum of `session_minutes.credit_ms`; agent time counts the distinct minutes with `ai_ms > 0`, so concurrent agents count once.
    - Languages: a minute's credit is split across `minute_languages` in proportion to the characters changed; with a language filter only that language's share counts (agent time too). Tasks: each minute counts for `session_minutes.task`.
  - `GET /api/projects` (`dashboard.rs`): each project with its tracked time, session count and prompts sent (all time), `repo_url`, `repo_auto` (found on GitHub, not set by the user) and `demo_url`. Projects group `agent_sessions` by folder name. 401 without a session.
  - `POST /api/projects/settings` (`projects.rs`): `{ name, repo_url, demo_url }` (each link `null` or an `http(s)://host…` URL, at most 500 characters; 400 otherwise); `Origin` must match `PUBLIC_URL`; 404 unless one of the user's sessions reported that project. Changing or clearing the repository sets `repo_manual`.
  - `GET /api/github`, `POST /api/github/disconnect`, `GET /auth/github`, `GET /auth/github/callback` (`github.rs`): see "Projects and GitHub" below.
  - `GET /api/devices` and `POST /api/devices/unlink` (`devices.rs`): the user's linked devices, most recently seen first, each with its `os` (from its latest heartbeat that has one; `null` before v7), credited time and first/last seen; unlinking (`{ "device_id" }`, `Origin` must match `PUBLIC_URL`) sets `devices.user_id` to NULL (decided 2026-11-22: the device can't upload or make app requests until it logs in again; its heartbeats and credited time stay). 404 if it isn't the user's.
  - `GET /api/settings` and `POST /api/settings` (`settings.rs`): `{ "timezone": IANA name | null }` (GET also returns `public_profile`); `POST /api/settings/profile` `{ "public": bool }` shows or hides the public profile (`Origin` checked). Saving the time zone needs the `Origin` to match `PUBLIC_URL` and a zone Postgres knows (`pg_timezone_names`).
  - `GET /api/u/{id}`, `GET /api/u/{id}/dashboard`, `GET /api/leaderboard`, `GET /badge/{id}/{project}.svg` (`profile.rs`): see "Public profiles" below. `GET /api/me` also returns the user's `id`.
  - `GET /api/app/today?start_ms=…` and `POST /api/app/logout` (`app.rs`): signed app requests, see "Signed app requests" below.
  - `GET /api/heartbeats?offset=N` (`dashboard.rs`, debug): the heartbeats the user's devices uploaded, newest first, 100 per page, with `total`, exactly as stored (device, seq, hash, prev, sig, claimed and received time, offline flag, body), with what it was credited (`credit`: session id, multiplier, the heartbeat's attention, credit and AI time, formula).
  - `GET /health` returns `ok`.
  - Downloads and updates (`releases.rs`, decided 2026-11-24): the newest release of `RELEASES_REPO` (default `Creative-Difficulty/Sisyphus`) from GitHub's API (`GITHUB_API_URL`, also used for a fake GitHub in tests), cached 5 min (a failed lookup retried after 1 min, keeping the last known release). `GET /download/{platform}` (`windows`, `macos`, `linux-x86_64`, `linux-aarch64`) redirects to that file (installer, `.dmg`, AppImage; older releases fall back to their archives), else to the release page. `GET /api/releases/latest`: `{ version, platforms }`. `GET /api/app/update`: the release's `update.json` text, its signature and every file's download URL (404 without a manifest); public, since the app checks the signature itself.
  - Review (`review.rs`, `signals.rs`; decided 2026-11-23): the fraud squad's page `/review` ("Pulse"). Reviewers are users whose Slack ID is in `REVIEWERS` (comma-separated env var); `/api/me` says `reviewer: true` for them, everyone else gets 403.
    - `GET /api/review/users?q=` finds users by name, Slack ID or id (the 50 seen most recently without `q`), with devices, last seen, credited time and heartbeats of the last 7 days.
    - `GET /api/review/timeline?user=&from=&to=` (at most 7 days, by claimed minute): every heartbeat with its credit (per-second keys/mouse in v9/v10, slots, prompt and response seconds, injected input, files with working-dir vs claimed characters, offline/stale, and from v10 the client version, models and tool calls by kind), the flags from `signals.rs` and a summary.
    - `signals.rs` computes flags on the fly from the stored heartbeats, so better rules apply to old data too. Flag only, never applied to credit (decided 2026-11-23). Kinds: `typed_without_keys` (typed characters far above the key presses since the session's previous prompt, at most 10 min back), `prompt_without_input`, `scripted_prompts`, `injected_input`, `metronome_typing` (≥ 60 s of typing without a quiet second, per-second variation < 20 %), `superhuman_typing` (≥ 3 s at 20+ keys/s), `periodic_input` (sparse input at near-constant intervals in 10-min windows: jigglers) (these three need per-second counts, so they only fire on v9/v10 heartbeats; Pulse spreads a v11 slot's counts evenly over its 10 seconds), `instant_replies` (prompt less than half the skimming time, 60 chars/s, after a long response), `metronome_prompts` (6 reply delays in a row within ±1 s), `edit_claims_exceed` (the decided edit-claim check), `overlapping_devices`, `marathon` (6 h without a 10-min break), `stale`, `offline`. Repeats close together are merged with a `count`. `signals::tests` holds a simulated person who must raise nothing; keep it passing when tuning.
  - `GET /models/sisyphus-prompt-1.bin` (`web.rs`): the prompt model, embedded in the binary from `models/` (cached as immutable), and its base model's licence at `/models/LICENSE-blink-multilingual-64M.txt`.
  - The web page (`crates/server/web/`: plain HTML, CSS and JS, no framework) and the Fredoka fonts are embedded in the binary (`web.rs`).
    - It matches the client: pink light/dark theme (OS-following, sun/moon switch saved in `localStorage`), Fredoka, pink bubbles.
    - Logged out it shows "Download for <system>" (guessed from the browser; just "Download" on phones or when the release lacks it) and a quieter "Log in with Hack Club" pill, with "Other systems" under them; logged in, a download icon, the name and "Log out" in the header (the icon also on public profiles), and the dashboard. "Other systems" and the icon open a `<dialog>` with one bubble per system (this computer's first, deeper pink) linking to `/download/{platform}`, and the version.
    - Every page (dashboard, public profile, Pulse) fills the full window width with 32 px side padding (16 px on phones), no max-width (decided 2026-11-23). The dashboard: a sidebar at the left edge (a row of pills on phones), the content filling the space to its right. The sidebar switches between Overview (decided 2026-11-23: a row of pill-shaped filters (date range, project, agent, system, language, task; a set filter turns pink); the tracked time for them big, then agent time, prompt, project and session counts; a bar chart of tracked time per hour/day/week with agent time faint behind it; then pink bar lists of tracked time by project, language, task, agent and system, at most 8 each with the rest summed, where clicking a bar filters by it and clicking again clears it; filters live only in the page), Projects (one bubble per project with its tracked time, sessions and prompts, and at its right end round pink icon links (a git branch for the repository, a globe for the live demo) when set, opening in a new tab; the links follow the bubble's button since links can't nest in buttons, in a fixed-width slot so the times line up; clicking the rest one opens a `<dialog>` with "Repository" and "Live demo" buttons (when set, opening in a new tab), its all-time tracked time, agent time, sessions and prompts, the time chart, its languages, tasks, agents and systems as bars, its most changed files (bars with characters changed), and "Links": the repository and live demo fields with "Found on GitHub" under an automatic match, and Save), Devices (one bubble per linked device: an OS icon drawn for the site as inline SVG, the system's name, last seen, tracked time, and an × that asks in a `<dialog>` before unlinking it), Leaderboard (public profiles ranked by tracked time, with a date-range pill (week by default); your row deeper pink; each row links to `/u/{id}`; a note when your own profile is private), Settings (a time-zone picker listing the browser's zones, preselecting the saved one or the browser's own; says UTC is used until saved; a "Public profile" row with "View" and "Make private"/"Make public"; and, when the server has a GitHub app, a GitHub row: "Connect" (to `/auth/github`) or "Connected as @login" and "Disconnect") and, set a little apart and with a dotted outline, the debug view Heartbeats: every uploaded heartbeat as a bubble (session title, project, minute), "Show more" for older pages; clicking one opens a `<dialog>` popup with the stored record as JSON, keys in the scheme's order. The view is kept in the URL hash (`#overview` / `#projects` / `#leaderboard` / `#devices` / `#settings` / `#heartbeats`). The project popup also has a "README badge" section: style pills (Flat, Flat square, For the badge, Pink; the choice saved in `localStorage`), a preview `<img>` and the Markdown with "Copy".
    - `/u/{id}` serves the same page; `app.js` sees the path and shows that public profile instead (logged in or not, no sidebar): name, a date-range pill, tracked time big, agent time, prompts, projects, sessions, the time chart, bars by language, task, agent and system (not clickable), and the projects with their time and repository/live demo icon links. "This profile is private or doesn't exist" otherwise.
    - It refreshes every 10 s. Data is inserted with `textContent` only, never as HTML.
    - Every response carries security headers (`main.rs`): a strict Content-Security-Policy (no inline scripts or styles, which is why `theme.js` is its own file), `X-Frame-Options: DENY`, `nosniff`, `Referrer-Policy: same-origin` (not `no-referrer`: that makes browsers send `Origin: null` on form posts, which broke the logout form's Origin check), and HSTS on https.
  - `Dockerfile` at the repo root builds only the server (`rust:1.95-slim-trixie`, then `debian:trixie-slim`), with BuildKit cache mounts for crates and `target/` so rebuilds are incremental. Only ever build it on the production server; see the Docker rule above.
  - Not built: the struggle value, applying the edit-claim check or any other signal to credit, the mouse-jiggle rule.

### Public profiles (`crates/server/src/profile.rs`, decided 2026-11-23)

- Public by default, existing accounts included (`users.public_profile`, migration 0012); turned off in the website's Settings.
- `GET /api/u/{id}`: name, `public`, `own`, and each project with credited time > 0, its time and its `repo_url`/`demo_url`. `GET /api/u/{id}/dashboard`: `stats::compute` for that user (the Overview's stats and filters, in the owner's time zone) with `files` always empty. Both 404 when the profile is private, unless it's the viewer's own.
- Never public: session titles, file paths, devices, heartbeats. Project folder names and their links are public.
- `GET /api/leaderboard?range=today|week|30d|all` (default `week`, works logged out): public users with credited time in the range, by tracked time, top 100, with agent time and `me`; ranges follow the viewer's time zone (UTC logged out or unset).
- `GET /badge/{id}/{project}.svg?style=flat|flat-square|for-the-badge|pink`: "Sisyphus" on plum and the project's all-time tracked time on pink, drawn as SVG on the server (Verdana widths approximated); "not public" in grey for a private profile or unknown project. `Cache-Control: public, max-age=300`. Badges link to `/u/{id}`.

### Crediting (`crates/server/src/credit.rs`, decided 2026-11-21)

The attention formula is `sisyphus_common::attention` (`minute()`), shared by the server (credit) and the client (debug screen), so both always compute the same. Each heartbeat becomes one `session_minutes` row of its server-side session:
- `agent_sessions.id`: UUID-formatted SHA-256 of device id, agent and agent session id; named by the latest `session_name`, grouped into projects by `project`.
- Attention is one value per heartbeat (formula 4). With `activity` and at least one known slot: interaction in the agent's window makes the whole heartbeat 1.0: more than 5 key presses (`attention::AGENT_KEYS`) summed over the slots whose focus is `agent`, mouse ignored there (decided 2026-11-21). With `input_kinds` `unknown` (Wayland without the input helper, heartbeats before v5) any input in an `agent` slot counts instead. A sent prompt's slot is `agent`, so typing a prompt of 6+ characters guarantees 1.0. Otherwise it decays from the session's previous heartbeat (`attention_end`): with a 2 min half-life if `agent` held most of the six slots (staying), else 30 s (leaving; `unknown` and `testing` slots count as another app; `testing` earns nothing for now, decided 2026-11-21). It starts at 0 after a gap. The multiplier is the heartbeat's attention.
- Without `activity`, or with every slot `unknown`: a prompt gives 1.0, an interrupt 0.5, otherwise 0; nothing carries over.
- Only minutes with `agent_running` earn credit (no time after a turn ends); `ai_ms` is 60000 for them, kept apart from credit.
- `share_minute`: if a user's multipliers for one `minute_ms` add up to more than 1.0 (across all devices), each gets its share; `credit_ms = 60000 × multiplier / max(1, sum)`.
- Offline heartbeats count; `offline` is copied to the row. Stale ones (see `heartbeats.stale`) get multiplier, credit and AI time 0 and carry no attention to the next minute (formula 7).
- Rows carry `formula` (`FORMULA`). On start, `backfill` deletes rows of other formulas and credits every stored heartbeat of a linked device without a row, so bumping `FORMULA` rebuilds everything from the raw heartbeats. Bump it whenever the calculation changes.
- A duplicate minute of the same session is ignored (`ON CONFLICT DO NOTHING`).
- For the dashboard's stats (formula 5, decided 2026-11-23), each row also stores the heartbeat's `os` (`''` before v7) and `task`: the top of the 13 task scores (`credit::TASKS`, after the 9 acts) of the minute's last scored prompt, else carried from the session's previous row (`''` before its first scored prompt). `minute_languages` holds the minute's `working_dir` changes (added + removed) per language (`language.rs`: by file name, then extension; unknown ones are "Other"); a minute without changes has no rows. Formula 6 (decided 2026-11-23) adds `prompts` (the heartbeat's `prompts.submitted`) and `minute_files` (each changed file's relative path with its `working_dir` added + removed).

### Deployment (`deploy/`)

Production runs on a Hetzner Ubuntu server at `2.29.187.1`, reached with `ssh -i ~/.ssh/sisyphus-ssh root@2.29.187.1`.
- Public at `https://sisyphus.aelx.dev`. The DNS record is in Cloudflare (aelx.dev), an `A` record set to DNS only (not proxied), so Caddy can get its own certificate.
- `deploy/compose.yaml`: Postgres 17 (named volume, not published), the server (not published), and Caddy (`deploy/Caddyfile`) on 80 and 443. Caddy gets and renews a Let's Encrypt certificate for `DOMAIN`, redirects http to https and proxies to `server:8080`. Run from the repo root with `--env-file /opt/sisyphus/sisyphus.env`.
- `/opt/sisyphus/sisyphus.env` (root-only, never in the repo) holds `POSTGRES_PASSWORD`, `PUBLIC_URL` (`https://sisyphus.aelx.dev`), `DOMAIN` (`sisyphus.aelx.dev`), `HACKCLUB_CLIENT_ID` and `HACKCLUB_CLIENT_SECRET`. The Hack Club Auth app must list `PUBLIC_URL` + `/auth/callback` as a redirect URI.
- The repo is cloned to `/opt/sisyphus/app`.
- Auto-deploy: `sisyphus-deploy.timer` runs `deploy/deploy.sh` every minute.
  - The script fetches `main`, and if it differs from the last successfully deployed commit (`/opt/sisyphus/deployed-commit`), it resets the clone and runs `docker compose up -d --build`.
  - A failed build is retried on the next run. `deploy.sh --force` redeploys regardless.
- Logs: `journalctl -u sisyphus-deploy` (deploys), `docker compose -f deploy/compose.yaml logs server` (the app), `... logs caddy` (HTTPS and certificates).
- The firewall (ufw) allows only 22, 80 and 443 (443 also UDP, for HTTP/3). Note that Docker-published ports bypass ufw.
- Pushing to `main` deploys. Don't push untested server changes.

### Accounts (Hack Club Auth)

Hack Club Auth (OpenID Connect) requires the client secret on every token request, so **only the server is an OAuth client**; the website and the app both log in through it. Hack Club tokens are used once to learn who logged in and are never stored.

- Website (`auth.rs`, `oidc.rs`):
  1. `/auth/login` stores a one-time login attempt (random `state` stored as a hash, `nonce`, PKCE verifier; 10 min) and sets a short-lived `login` cookie holding the state, scoped to `/auth/callback`.
  2. It redirects to Hack Club Auth with scopes `openid profile slack_id` (no email, no verification status) and PKCE S256.
  3. `/auth/callback`:
     - checks the cookie matches `state`, then deletes the attempt so it can't be replayed
     - exchanges the code (with the secret and PKCE verifier) and verifies the ID token: RS256 against the provider's keys (cached, re-fetched on an unknown key id), issuer, audience, expiry, nonce
     - upserts the user
     - sets a session cookie: 256-bit random, only its SHA-256 stored, 30 days, `HttpOnly`, `SameSite=Lax`, plus `Secure` and the `__Host-` prefix on https
  4. `POST /auth/logout` requires the `Origin` header to match `PUBLIC_URL`. `GET /api/me` returns the name.
- Desktop app:
  1. The app listens on a random `127.0.0.1` port and opens `/auth/app?port&challenge&device`, where `challenge` = SHA-256 hex of a random verifier and `device` = its public key.
  2. After the same Hack Club login, the callback shows a confirmation page (`/auth/app/confirm`, tied to the browser by a cookie scoped to that path): "Log in the Sisyphus app on this computer as <name>?" with "Log in" and "Cancel" (decided 2026-11-22, so a login link followed by accident links nothing; Hack Club Auth may skip its own screen). Both are form posts checked against `PUBLIC_URL` (`Origin`). Only "Log in" redirects the browser to `http://127.0.0.1:<port>/done?code=…`; that page has its own CSP allowing `form-action http://127.0.0.1:*`, since browsers apply `form-action` to the redirect. The code is one-time and stored hashed (`app_codes`, `confirmed`, `app_port`): 10 minutes to confirm, then 2 minutes to exchange; `/auth/app/token` only accepts confirmed codes. Cancel deletes it (the app keeps waiting until its own timeout).
  3. The app posts `AppTokenRequest` to `/auth/app/token`: the code, the verifier, and a device signature over `device_link_message(code)` (`sisyphus-common::api`).
  4. The server checks all three and links the device to the user. A device linked to another account can't be taken over (409).
  - The app gets no bearer token. Uploads are already signed by the device key (and a replayed batch doesn't continue the chain), and the server accepts them only from linked devices. Everything else the app asks for is a signed request (below).
- Discovery endpoints must be https; plain http is only accepted when `HACKCLUB_AUTH_URL` is itself `http://localhost` / `http://127.0.0.1` (local testing).
- "Log out" in the app first unlinks the device on the server (`POST /api/app/logout`) and only forgets the account once the server agreed (or says the device isn't linked); if the server can't be reached it stays logged in and shows the error. Logging in again links it again.
- Devices can also be removed (unlinked) on the website's Devices view; the app then gets 403 on uploads and app requests and keeps its queue until it logs in again.
- Not built yet: rate limiting. Switching servers in settings leaves the device linked on the old server.

### Projects and GitHub (`github.rs`, `projects.rs`, decided 2026-11-23, modelled on Hack Club's Hackatime)

- Nothing new leaves the user's machine: projects are matched to repositories by their folder name on the server. The client never sends git remotes.
- Connecting: `/auth/github` (logged in) redirects to GitHub's OAuth with no extra scopes (public data only) and a random `state` in a cookie scoped to the callback; `/auth/github/callback` checks it, exchanges the code (with the client secret), reads `/user` and stores id, login and token on the user. A GitHub account belongs to one Sisyphus account at a time (connecting it elsewhere clears it here). Connecting resets earlier misses and wakes the matcher. `POST /api/github/disconnect` (`Origin` checked) deletes the token and asks GitHub to revoke the grant (`DELETE /applications/{client_id}/grant`, best effort); repositories already found stay.
- The matcher (a tokio task, every 10 min or when woken): for each connected user's projects without a repository and not set by hand (`repo_manual`), whose name is a valid GitHub repo name: `GET /repos/{login}/{name}`, public → linked; else the user's public organisations (`/users/{login}/orgs`), where the repo is public and has a commit by the user (`/commits?author=`), linked only if exactly one organisation matches (more than one: left unlinked); else the public repos the user contributed to (GraphQL `repositoriesContributedTo`, `privacy: PUBLIC`, commits and pull requests, not their own; GitHub's view covers the past year), linked if exactly one has the project's name (ignoring case). Orgs and contributed repos are fetched once per user per pass. Private repos are never linked. A miss is retried after 14 days, or on the next pass when it was made by an older matcher (`projects.repo_checked_by` < `github::MATCHER`; bump `MATCHER` whenever the matcher can find more, like `FORMULA` for crediting). At most 50 GitHub requests per user per day (in memory). 401 deletes the token; 403/429/5xx stop that user until the next pass.
- The token is stored in plain text in `users.github_token`, as Hackatime does.
- Testing: a fake GitHub on `127.0.0.1` (OAuth authorize/token, `/user`, `/repos/…`, `/users/…/orgs`, commits, the grant DELETE) with `GITHUB_URL` / `GITHUB_API_URL` pointing at it; follow the redirects with curl like the Hack Club login. Popups can be screenshotted by driving headless Chromium over the DevTools protocol (`node --experimental-websocket`, `Runtime.evaluate` e.g. `openProject('name')`, `Page.captureScreenshot`).

### Signed app requests (decided 2026-11-21)

Only a logged-in app (a device linked to an account) may upload or read data. The app has no token; it signs each request with its device key (`sisyphus-common::api`):
- Headers `sisyphus-device` (public key, hex), `sisyphus-time` (unix ms) and `sisyphus-signature` (hex ed25519 signature over `request_message`: domain tag `sisyphus/request/v1`, method, path with query, time, SHA-256 of the body).
- The server (`app::verify`) refuses with 401 a missing or invalid signature, a time more than 5 min from its clock, or a signature it has already seen (an in-memory cache, `SeenRequests`, kept for twice the allowed skew); 403 if the device isn't linked.
- The app's clock must therefore be roughly right; it says so when the server answers 401.
- This proves the request came from that device, not that the client is unmodified: the key lives on the user's machine. Truthfulness is checked server-side.

### Agent integrations (`crates/client/src/agents/`)

`mod.rs` defines the plugin interface:
- `AgentIntegration` discovers sessions (`get_instances`), and has `install()` / `is_installed()`. Its signature was written by hand; extend it rather than restructuring it.
- `AgentInstance` is one session: title, uuid (the agent's session id), project path, last activity, `is_running()`, `read_events()`, and `attach(AttachMode)`.
- `attach` returns an `Attachment`, which wraps a worker thread and an mpsc channel of `AgentEvent`s; dropping it stops the worker.
- `AgentEventKind`:
  - `Prompt { text, pasted_chars, scripted }`, `Command`, `Response`, `Thinking`; `scripted` is set from Claude Code's `entrypoint` (`sdk-cli`, `sdk-ts`, `sdk-py`: the Agent SDK and `claude -p`), false elsewhere
  - `ToolCall` and `ToolResult` (outcome: success / error / rejected by the user)
  - `Interrupt`, `TurnEnd`, `Compacted`, `TitleChanged`
  - `Model { name }`: the model the next output comes from (Claude Code: `message.model` on every assistant line, skipping `<synthetic>`; opencode: `model.id` of each assistant row, `modelID` in v1; Codex: `model` of each turn's `turn_context` line)
- `Integration` is an enum over all integrations (`Integration::ALL`), with `id()`, `name()`, `instances()`, `instance(id)`, `is_installed()` and `install()`, so callers can treat every agent the same. `id()` values are stored in the config; never change an existing one.
- `is_running()` means the agent is mid-turn: the newest event isn't a turn end or interrupt (Claude Code), the newest prompt/reply row leaves the turn open (opencode), or the newest turn marker is `task_started` (Codex). All also require activity in the last 15 min, so a crashed session doesn't stay "running".
- `install()` does nothing yet beyond checking the agent is present, because all integrations only read files. It is the hook for integrations that will need something set up in the agent.

No integration installs hooks. Each reads the agent's own on-disk storage, whose format is undocumented: skip unknown lines and fields rather than erroring.

- **`claude_code/`** reads the append-only JSONL transcripts at `~/.claude/projects/<escaped cwd>/<session uuid>.jsonl` (or `$CLAUDE_CONFIG_DIR`; falls back to `USERPROFILE` on Windows).
  - `transcript.rs` maps lines to events. It filters `isMeta`, `isSidechain` and compaction-summary lines, and lines whose `origin.kind` is not human. Interrupts are `[Request interrupted by user…]` text. Rejected tool calls are recognised by `toolUseResult`.
  - Sessions are listed by scanning only the head and tail of each file, because transcripts reach hundreds of MB and title lines are re-appended.
  - Live following uses `tail.rs`, a polling tail. Claude Code flushes its transcript about a second late.
- **`opencode/`** reads opencode's SQLite database (`$XDG_DATA_HOME/opencode/opencode.db`), read-only, using the `session_v2` and `session_message` tables.
  - Older opencode versions (such as 1.4) only have the v1 `session`/`message`/`part` tables. `Schema::detect` picks v1 only when `session_v2` is missing (newer versions keep the migrated v1 tables around), and `v1.rs` rebuilds each v1 message and its parts into the v2 row shape, so `session.rs` parses both.
  - Assistant rows are rewritten in place while streaming. Attachments therefore poll by `time_updated` (`>=` watermark), and `session.rs` deduplicates by remembering which events it already emitted.
  - The database and prompt-history paths only use `XDG_*` / `HOME`, so opencode isn't found on Windows yet.
- **`codex/`** reads Codex's append-only rollouts at `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<local time>-<thread id>.jsonl` (`CODEX_HOME`, else `~/.codex`; on Windows the profile folder before `HOME`). Every format claim is backed by Codex's source at a release tag in `.scratch/codex-integration/evidence.md` (local, git-ignored).
  - Each line is `{timestamp, type, payload}` (since v0.32; older files are skipped). The first line is `session_meta`: thread id, `cwd`, `source`, `thread_source`. Only threads the user drove are listed: `thread_source` absent or `user`, and `source` `cli`, `vscode` or missing (before v0.44). Sub-agents, `/btw` side chats, `codex exec` and internal threads are skipped. One instance per thread id, newest file wins (reverts write a second file); `.jsonl.zst` (compression, off by default) and `archived_sessions/` aren't read.
  - `rollout.rs` parses both formats, which never mix in one file: legacy (`event_msg` `user_message` / `agent_message` / `agent_reasoning`) and paginated (default for new threads since v0.151: `event_msg` `item_completed` with `UserMessage` / `AgentMessage` / `Reasoning` / `CommandExecution` / `FileChange` / `McpToolCall`). Tool calls are `response_item` `function_call` / `custom_tool_call` / `local_shell_call` / `web_search_call`; a result's outcome comes from the matching `item_completed` status, else from the output text (rejections, aborts, exit codes in the three shapes Codex has used). `task_complete` is a turn end (since v0.100), `turn_aborted` with `interrupted` an interrupt (Esc or a cancelled approval), the `compacted` line a compaction, a `user_shell` command (`!` in the prompt) a `Command` named `!`. Injected context (`response_item` messages) is ignored.
  - Legacy forks (v0.81–v0.150) copy the parent's history, parent `session_meta` included, with fork-time timestamps; the parser skips that block (from a second `session_meta` to the first gap of more than 1 s between lines).
  - `titles.rs`: names from `$CODEX_HOME/session_index.jsonl` (`{id, thread_name, updated_at}`, since v0.93; newest entry wins, an empty name clears it), else the first prompt. While attached, the index is re-read when it changes (through `tail::follow_lines_with_idle`) and renames arrive as `TitleChanged`.
  - Live following uses `tail.rs`; Codex flushes each line as its item completes (measured lag about 0.1 s). "Waiting for approval" isn't written anywhere.
  - Pasted characters are always `None`: Codex expands pastes into the prompt before saving it.
- `Prompt.pasted_chars` comes from each agent's prompt history, not the session itself:
  - `claude_code/history.rs` reads `~/.claude/history.jsonl`, matched by session id and timestamp, with a cross-session fallback for resumed sessions.
  - `opencode/history.rs` reads `$XDG_STATE_HOME/opencode/prompt-history.jsonl`, matched by the expanded prompt text. Its placeholder offsets may be UTF-16, so placeholders are found by text.
  - Only large pastes are recorded there, so small pastes count as typed. `None` means the prompt wasn't in the history: queued prompts, or opencode prompts older than its ~50-entry history.
  - Only lengths (and a hash, for opencode) are kept, never pasted text.

To add an agent:
1. Add a submodule implementing both traits.
2. Add an `Integration` variant and its arms in every `Integration` helper, including a new `id()`.
3. Add its logo SVG to `crates/client/assets/` and an arm in `gui::style::logo`.

### Activity (`crates/client/src/activity/`)

Reads, without a keyboard hook, the time since the last input (any key or mouse event; never which key), how many key presses and mouse events happened, and the focused app. The tracker records it; the `activity` CLI command prints it.

- Activity is always read (decided 2026-11-21: no consent step, no settings row). `Monitor::new(true)`; `since_last_input()`, `take_input()` → key/mouse counts since the last call and their `InputKinds` (`None` when they can't be told apart), `focused()` → `FocusReading` (`Unknown` / `Nothing` / `App(FocusedApp { app_id, pid })`), `describe()` (which backends are in use).
- Keys vs mouse (decided 2026-11-21):
  - macOS: exact, `CGEventSourceCounterForEventType` per event type (key down; mouse down/moved/dragged/scroll), deltas between calls. Injected input = the combined session state's count minus the HID (hardware) state's. Events an app posts at the HID level look like hardware, so this only catches some autotypers (tested 2026-11-23: zero-distance moves posted to `kCGHIDEventTap` counted as hardware).
  - Windows and X11: inferred (`sampler.rs`): a thread polls the last input time (`GetLastInputInfo` / XScreenSaver), the cursor (`GetCursorPos` / `QueryPointer`) and the mouse buttons (`GetAsyncKeyState` for mouse buttons only / the pointer's button mask) 25 times a second. New input with the cursor still and no button down is a key event; key events are halved into presses (releases reset the idle time too). Clicks and scrolls without movement count as keys.
  - Linux Wayland: exact through the input helper (`linux/helper.rs` connects to `/run/sisyphus-input/socket`, reconnecting every 5 s); unknown without it. `activity::input_helper_missing()` says whether to show the setup popup.
- Input helper (`input_helper.rs`, Linux only, decided 2026-11-21): `sisyphus-client input-helper install|restart|uninstall` (as root through `pkexec`, for `PKEXEC_UID`/`SUDO_UID`) copies the binary to `/usr/local/libexec/sisyphus-input-helper`, writes `/etc/systemd/system/sisyphus-input-helper.service` and enables it; `uninstall` removes both. The service runs `input-helper serve --uid N` as a `DynamicUser` in the `input` group (no capabilities, no network, `DeviceAllow=char-input r`, `ProtectSystem=strict`), reads `/dev/input/event*` (rescanned every 5 s), counts key presses (value 1, codes outside the button range; no repeats or releases) and mouse events (relative-motion frames, button presses, absolute frames only while `BTN_TOUCH`), and every 250 ms with anything counted writes `<keys> <mouse> <virtual keys> <virtual mouse>\n` to connections from that uid only (`SO_PEERCRED`). The virtual counts come from devices under `/sys/devices/virtual/input` (uinput: ydotool and other autotypers, but also key remappers like keyd and kanata) and are included in the first two; the client reads them as `injected` and accepts helpers that send only two numbers. Key codes are only compared against the button range, never stored or sent.
- `FocusReading::focus(session, &processes)` → the heartbeat's `Focus` class (`agent`/`terminal`/`editor`/`browser`/`other`/`testing`/`none`; `None` = unknown). `testing` (decided 2026-11-21): the window runs an executable inside the session's project folder, which must be a git repository (`process::git_root`: has `.git`). `Processes::programs_in_window` collects the executables of the window's process and its descendants, stopping at agent processes (what an agent runs isn't the user testing); executable paths are read only for those (`process::exe_path`: `/proc/<pid>/exe` without a ` (deleted)` suffix, `proc_pidpath`, `QueryFullProcessImageNameW`). Without a window pid, processes named like the app id stand in for the window. `classify.rs` maps app ids (Wayland app ids, X11 classes, macOS bundle ids, Windows exe names) to classes; unknown apps are `other`.
- `agent` matching (`process.rs`): the focused window runs the session's agent if an agent process (`claude`, `opencode`, Codex's native `codex` binary (`codex-<triple>` up to v0.40), or node/bun/deno running Claude Code, opencode or the `@openai/codex` launcher) running that session descends from the window's process. Codex has its own list of non-session subcommands (from its clap `Subcommand` enum: `app-server`, `exec`, `review`, `mcp`, `login`, …; `resume` and `fork` are sessions), found after skipping its global options and their values; Codex names no session per process, so its match uses the working directory. A Claude Code process names its session in `~/.claude/sessions/<pid>.json` (`sessionId`); where that file exists the match is exact, otherwise the process's working directory must be the session's project. Subcommands that aren't sessions (`claude agents`, `claude daemon`, `claude mcp`, `opencode serve`, …) never count, and are never the session's agent. Claude Code's native builds run as `…/claude/versions/<version>` (process title `claude …`) and are recognised by that path. Command lines are read for `claude`/`opencode` and node/bun/deno.
- tmux (decided 2026-11-23): the tmux server runs detached from every window, so an agent in a pane never descends from the terminal. When a `tmux` process and an agent run, `Processes::snapshot` asks the default tmux server (`list-panes -a`, `list-clients`) which panes are on screen (the active pane of its session's active window) and in which clients; an agent counts as in the window when its pane is on screen in a tmux client that descends from the window. Other tmux sockets (`-L`), screen and zellij aren't handled.
- Session manager (`claude agents`): it shows background sessions (`kind: "bg"` in the pid file) without telling us which one. When the focused window runs it, the background session that got the latest prompt (falling back to the latest activity) is in front (decided 2026-11-21; `Processes::window_runs_manager`, `background_sessions`, picked in the tracker). Without a window pid (wlr-foreign-toplevel) an ancestor's executable must match the app id, which can't tell two windows of the same terminal apart. Windows can't read other processes' working directories, so there any process of that agent counts.
- A focused window without an app id but with a pid is still a window (`app_id` empty, classed `other`), not "nothing in front". Our own window sets the app id `dev.aelx.Sisyphus` (`gui::window_settings`; Wayland app id and X11 class).
- `app_id` and `pid` never leave the machine; only the `Focus` class may go into heartbeats. Window titles are never stored (sources that send them, like Hyprland or wlr, have them dropped on parse).
- Backends:
  - Windows (`windows.rs`): `GetLastInputInfo`; `GetForegroundWindow` → process → exe name; Toolhelp process table.
  - macOS (`macos.rs`): `CGEventSourceSecondsSinceLastEventType`; `NSWorkspace.frontmostApplication` (bundle id, pid; kept current by the window's main run loop, so the CLI may see stale values); libproc process table and cwd, `KERN_PROCARGS2` for node command lines. No permission prompt needed.
  - Linux X11 (`linux/x11.rs`): XScreenSaver; `_NET_ACTIVE_WINDOW`, `WM_CLASS`, `_NET_WM_PID`.
  - Linux Wayland: idle from `ext-idle-notify` v2 input notifications (ignores idle inhibitors; 1 s resolution), or Mutter's IdleMonitor over D-Bus on GNOME. Focus, first that applies: Hyprland IPC (`j/activewindow`), Sway IPC (`get_tree`), GNOME via the third-party "Window Calls" Shell extension (`org.gnome.Shell.Extensions.Windows.List`; unknown without it), KDE via a KWin script loaded over D-Bus that calls our bus name `dev.aelx.Sisyphus` with the active window's class and pid, then `wlr-foreign-toplevel-management` (niri, river, labwc, Wayfire, …). The Wayland connection runs on its own thread, stopped when the `Monitor` is dropped.

### Tracker (`crates/client/src/tracker/`)

Records heartbeats (v11) and uploads them. The app starts it (`Tracker::start`) and it runs until the app quits, also while the window is closed to the tray; there is no background service yet. The `track` command runs it without the window.
- One tracker per machine: `tracker.lock` next to the config is locked (`File::try_lock`); a second window or `track` doesn't record.
- `mod.rs`, once a second:
  - every 10 s re-reads the config (connected agents, consent; the `Monitor` is recreated when consent changes) and the session list of the connected agents
  - follows (attaches to) sessions that are running, or were active or in front within 30 min, with `AttachMode::Since(ms)` so events from just before attaching aren't missed
  - samples input and focus into the open minute. Among sessions of the same agent and project whose agent runs in the focused window, the most recently active one gets `agent`; the others get the window's plain class. A session whose project is a git repo gets `testing` when the window runs one of its programs (`Session::git_root`, refreshed with the session list), and the program's relative path goes into the minute's `tested_programs`
  - remembers each second's window reading for the open minutes (plain class plus the agent processes in the window, `Processes::agents_in_window`). When a session is first followed (opencode creates a session only with its first prompt, and sessions are listed every 10 s), its slots without readings are filled from this history (`backfill`), so typing the first prompt counts
  - a prompt submitted to a session marks its slot as `agent` for that session (`SessionMinute::prompt_slots`; a prompt means the user was focused on it), but only when the slot's window reading could be the agent's (terminal, editor or unknown), not a browser, another app, a test program or nothing (decided 2026-11-23: otherwise a scripted prompt made typing anywhere count as typing to the agent). Prompts the agent marks as scripted never mark slots. Input still comes only from the OS
  - input is counted per slot (`MinuteInput::add_counts`), with the injected part; each prompt's and finished response's second goes into `prompts.at` / `responses.at` (late events: second 0 of the current minute)
  - counts prompts (typed = length − pasted), responses (visible characters), and edit claims (on a successful tool result)
  - tool calls by kind (`tools.rs`: `shell`, `edit`, `read`, `web`, `agent`, `other`, by tool name for all agents; a call with edit claims is `edit`): counted in the minute the call starts, its duration (call to result, by the agent's timestamps, so including approval time) added in the minute the result arrives (decided 2026-11-25). The model last named by a `Model` event goes into each minute with a response, thinking or tool call (`models`, at most 100 characters each). Every heartbeat carries the client's version (`client`)
  - every 10 s, looks for the projects' own programs running (`Processes::programs_running`: every process's executable, skipping agent-started ones; the tracker's own process counts too, decided 2026-11-21) and gives each git project's programs to its most recently active session (`Session::programs`), which is then followed and records them as `running_programs` each second (decided 2026-11-21)
  - closes minutes 5 s after they end. A session's minute becomes a heartbeat when its agent ran, its window was in front for most of a slot, anything happened in it (decided 2026-11-21), or the project's program ran. Without consent, `activity` is false and slots are `unknown`.
  - Events for an already closed minute go into the current one.
- `files.rs`: `ProjectWatch` polls a project folder every 2 s while one of its sessions runs (and 10 s after). Each line is kept only as a hash of its non-whitespace characters plus their count; changes are counted as non-whitespace characters of lines added/removed (common prefix/suffix stripped, then a multiset difference). The first poll after arming is a baseline that doesn't count. Skips dependency/build/VCS folders, files over 1 MB, non-UTF-8 or binary files, symlinks, and stops at 20,000 files. A change goes to the session that last claimed an edit to that file, otherwise the most recently active running session in the folder. Because the working directory is counted in whole lines and claims count only the replaced text, `working_dir` is usually somewhat higher than `agent_edits` for edits inside a line; claims above `working_dir` are the suspicious direction.
- `edits.rs`: edit claims from Claude Code's `Edit`/`MultiEdit`/`Write`, opencode's `edit`/`write` and Codex's patches, counted the same way. A `Write` is compared with the file's version before the write, from the watch. Codex patches (grammar in `codex-rs/core/assets/tools/apply_patch.lark`) come from its `apply_patch` tool or a heredoc in its shell tools (`exec_command`, `shell`, `shell_command`, `local_shell`, `container.exec`, optionally after `cd <dir> &&`): added files are writes, updated files replacements of their `-`/`+` lines (a move claims the new path), deleted files writes of nothing. Paths outside the project are dropped; other tools (plain shell commands) only show up in the working directory.
- Known gap (any agent): sessions are re-read every 10 s, so the first turn of a brand-new session can finish before its project folder is watched, and its `working_dir` changes are missed.
- `minute.rs`: a session's minute; a slot's focus is the class seen most often (latest on a tie).
- `queue.rs`: heartbeats are signed with the device key into the chain, appended to `queue.jsonl` (written before the tip) and the tip saved in `chain.json`, both next to the config. The uploader thread sends batches of up to 500 every 30 s while logged in (`account()`) and the server is https or localhost, never holding the queue's lock during a request. On `OutOfSync` it drops what the server already has, or signs the unsent heartbeats again from the server's tip if the chains differ. At most 50,000 are kept.
- Prompts are classified by the prompt model as they arrive (once it's downloaded; the tracker loads it on its 10 s reload): each prompt's per-category scores go into the minute's `prompts.classes`, `prompts.model` names the model, and `corrections` counts prompts scored at 500 or more as `act:correction`. The text is dropped right after.
- Not built: opencode on Windows, tmux/terminal-tab disambiguation beyond "most recently active".

### Prompt model (`crates/client/src/prompt_model/`, decided 2026-11-22)

- Gives every prompt a probability (0.0–1.0, sent as 0–1000) for each of 22 categories, in `LABELS` order: 9 acts (`new_request`, `correction`, `clarification`, `approval`, `question`, `stop_undo`, `handoff`, `chit_chat`, `other`), then 13 tasks (`feature`, `bug_fix`, `error_paste`, `refactor`, `understand_code`, `learn_concept`, `ui_styling`, `tests`, `setup_deploy`, `docs_writing`, `planning_design`, `review`, `other`). Definitions: `tools/prompt-model/labels.md`. Categories are independent (sigmoid each).
- The model: `minishlab/blink-multilingual-64M` (MIT; static embeddings, 250,000 word pieces x 256, 100+ languages), its table and a 256 → 512 → 512 → 22 classifier fine-tuned on our labels. Shipped as `models/sisyphus-prompt-1.bin` (26.7 MB, our own format, see `mod.rs` docs): the original table as 64 one-byte PQ codes per row, the ~29,000 fine-tuned rows and the layers as int8, the tokenizer, and the Unicode tables it needs.
- Privacy: fine-tuning only changed rows of word pieces that occur in the public, LLM-written training data; private real prompts trained the classifier but no word only they contain, so the file doesn't reveal their vocabulary.
- Memory: the file stays open and only the rows a prompt uses are read from it (`classify` returns `None` if that fails); in memory are a compact sorted word index (`tokenizer::StrIndex`, about 5 MB), the codebooks and the int8 layers: about 6–9 MB in all. The SHA-256 is checked while streaming the file on open.
- Inference in plain Rust, no new dependencies (`tokenizer.rs` matches Hugging Face `tokenizers` for this tokenizer: added tokens first, `BertNormalizer` with lowercase, `BertPreTokenizer`, 100-char pieces, WordPiece): tokenize, drop `[UNK]`, average rows, L2-normalize, layers with ReLU, sigmoid. Under a millisecond per prompt.
- Not bundled: downloaded on first start ("Additional required files") from `<server>/models/sisyphus-prompt-1.bin` into `models/` next to the config; used only if size and SHA-256 match `MODEL_SIZE`/`MODEL_SHA256`, also checked on every load. A new model needs a new name, hash, parity file and server route.
- `prompt_model::tests` checks 391 cases against the Python reference (`crates/client/tests/data/prompt_model_parity.json`): token ids exactly, probabilities to 0.0005. It reads `models/sisyphus-prompt-1.bin` from the repo.
- Training: `tools/prompt-model/` (scripts, synthetic data, README with results; real prompts are never committed). Results: real prompts (one person) act 71% / task 55% right; synthetic 12 languages 68% / 69%; with typos 58% / 59%.

### Updates (`crates/client/src/update.rs`, decided 2026-11-24)

- Only installed copies update: the Windows installer's (`unins000.exe` next to the exe), a macOS `.app`, a Linux AppImage (`$APPIMAGE`), and only where that location is writable. Development builds and the bare Linux binary never do.
- The window's `Updater` thread checks `GET /api/app/update` 30 s after start and then every 6 h (1 h after a failure). `update.json` (`{ kind: "sisyphus-update/1", version, files: { <platform>: { name, sha256, size } } }`, written by `packaging/update-manifest.py`) must be signed (ed25519 over its exact bytes, `update.json.sig` in hex) by the release key: `RELEASE_PUBLIC_KEY` in `update.rs`, private half in the repository's `RELEASE_SIGNING_KEY` Actions secret (a backup is on the maintainer's machine, never in the repo). The release job refuses to sign if the secret doesn't match `RELEASE_PUBLIC_KEY`. Only a strictly newer `x.y.z` is taken, so old manifests can't roll back. The platform's file is downloaded into `update/` next to the config and kept only if size and SHA-256 match; it's hashed again before installing.
- Installing is silent, at a quiet moment (checked every 10 s refresh): window and panel closed and no followed session running. Windows runs the new setup with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /relaunch=1` (it closes the app, keeps the earlier choices and starts it again with `--background`); macOS unpacks the `.app.tar.gz` with `/usr/bin/tar` next to the bundle and swaps the bundles; Linux replaces the AppImage file. On macOS and Linux a `sh -c 'sleep 3; …'` starts the new version with `--background` after this one quit (the tracker lock must be free). Then the app exits.
- Not built: code signing, updating the Linux input helper's installed copy (its protocol is stable), auto-start at login on macOS and Linux (Windows has it in the installer).

### Packaging (`packaging/`, decided 2026-11-24)

- Windows (`windows/sisyphus.iss`): per-user install without admin rights into `%LOCALAPPDATA%\Programs\Sisyphus\Sisyphus.exe`, Start menu entry, optional desktop icon, "Start Sisyphus when I log in" (checked by default; `HKCU\…\Run` with `--background`), uninstaller that stops the app first. Never change its `AppId`.
- macOS (`macos/app.sh`, `Info.plist`, `icon-1024.png` → `.icns`): `LSUIElement` (menu bar app); `tray::show_in_dock` makes it a regular app with a Dock icon only while the window is open (decided 2026-11-24).
- Linux (`linux/appimage.sh`, `sisyphus.desktop`): only the binary goes in. Inside an AppImage, root can't read the FUSE mount, so the input helper's `pkexec` runs `$APPIMAGE --appimage-extract-and-run input-helper …` instead of the mounted binary.
- Downloaded build tools (appimagetool) can't be run locally by agents here; test packaging with a dry run of the release workflow.

### Local config (`config.rs`)

JSON in the OS config directory:
- Linux: `$XDG_CONFIG_HOME/sisyphus/config.json`
- macOS: `~/Library/Application Support/sisyphus/`
- Windows: `%APPDATA%\sisyphus\`

Fields:
- `connected_agents`: a list of `Integration::id()`s.
- `dark_mode`: the window's light/dark choice; absent means follow the OS.
- `server_url`: the Sisyphus server; absent means production, `https://sisyphus.aelx.dev` (`DEFAULT_SERVER_URL`). Changeable in Settings.
- `account`: `{ "name": …, "server": … }` once logged in, absent when logged out. A device is linked per server, so `Config::account()` only returns the account when its `server` matches the current one; switching servers (or an older account without `server`) means logging in again. Always use `account()`, not the field.

`device.key` (the device's private key) lives next to it, as do `update/` (a downloaded update), the tracker's `queue.jsonl`, `chain.json` and `tracker.lock`, and `models/sisyphus-prompt-1.bin` (the downloaded prompt model).

Unknown fields are ignored. Saving replaces the file atomically.

### Window (`crates/client/src/gui/`, iced 0.14)

- Rendered in software (tiny-skia; iced's `wgpu` feature is off, decided 2026-11-22): the GPU renderer loaded the graphics drivers (LLVM, Mesa, Vulkan, about 135 MB) into the process. Measured in release builds: window about 70 MB on the dashboard, `track` about 28 MB.

- macOS: a Dock icon only while the window is open (`tray::show_in_dock`, decided 2026-11-24).
- Runs as an `iced::daemon` and lives in the tray (decided 2026-11-24): closing the window closes it (freeing its memory) but keeps the app, the tracker and the 10 s refresh running; the only way to quit is "Quit" in the tray menu. The window opens at start once the OS theme is known (otherwise tiny-skia redraws only part of the first frame after the theme arrives).
- Tray (`crates/client/src/tray/`): Linux through StatusNotifierItem over D-Bus (`ksni`, on zbus; no GTK), Windows and macOS through `tray-icon` (created and updated on the main thread, from `update`). Hovering shows "Sisyphus" and the info lines; the menu shows the same lines greyed out, then "Open Sisyphus" and "Quit". A left click opens the window on Linux; on Windows and macOS it shows the panel (decided 2026-11-24) and the menu is on the right click. The lines (`App::update_tray`, refreshed every 10 s): today's tracked time and agent time from the server (or "Not logged in"), and the session an agent is working on ("Working: <title>", "No agent working", or "Not tracking" without a tracker). Never the upload queue (decided 2026-11-24). The panel (`gui/panel.rs`, Windows and macOS): a borderless, always-on-top second window (300 × 236, out of the taskbar, rounded with a shadow on Windows 11) with the logo, today's tracked time big, agent time, the working line and an "Open Sisyphus" button. It opens hidden near the icon (`tray::IconRect`, physical pixels), then `PlacePanel` moves it once its scale factor and screen size are known: centred on the icon, below it when the icon is in the screen's top half (the macOS menu bar), else above it (a taskbar), kept off the right edge. It closes when it loses focus or on a second click (a click within 400 ms of it losing focus doesn't reopen it). Wine's tray (Xvfb, no window manager) passes no clicks to `tray-icon`, so the panel can only be tested there by sending `Message::Tray(Event::Panel(…))` from the code temporarily. Without a tray (plain GNOME, no `StatusNotifierWatcher` on the bus) closing the window quits, as before.
- Logo (`crates/client/assets/logo.svg`, `logo.rs`): a deliberate placeholder (pink rounded square, dashed outline, faint cross, "S"; decided 2026-11-24) until a real one exists. Used as the window icon, the tray icon (drawn with `resvg`), above the name on the login step, and on the website as favicon and in the header (`/logo.svg`). Not yet: an icon embedded in the Windows `.exe`, a Linux `.desktop` file, a macOS app bundle.
- `mod.rs`: the app state, `Message`s and screens. `resume()` picks the screen from the config: not logged in → login step; prompt model missing → additional required files; no agents → connect step; Linux Wayland without a running input helper → the input helper popup; otherwise the dashboard.
- First start (`onboarding.rs`), never the settings screen:
  1. Login step: name, one line, "Log in with Hack Club". After logging in it shows the name and "Continue". Login is required.
  2. "Additional required files" (`files.rs`, decided 2026-11-22): a card "Downloading additional required files" (then "… downloaded") with a pink progress bar ("8.9 of 27 MB", then "Ready"). It never calls the file a model (decided 2026-11-22), and neither do the download's error messages. The download starts by itself from the server (`prompt_model::download`, on a worker thread, progress polled five times a second); "Continue" is enabled once it's saved and its SHA-256 checked; on failure the error and "Try again". Required: shown again whenever the file is missing.
  3. Connect step: agent tiles (logo, name, Connect/Disconnect) and "Continue", enabled once an agent is connected.
- Input helper popup (`input_helper.rs`, only on Linux Wayland without the helper running): says why (Wayland doesn't tell apps keys from mouse), what it does (counts only, never which keys), what the password allows (the installed copy, the service, `/dev/input`, not root, no network, only this account reads it), how to remove it, and "Questions or concerns? Contact @simon on Slack." "Set up" runs `pkexec … input-helper install` on a worker thread ("Waiting for your password" meanwhile; errors for cancel, no polkit agent, failures); it can't be skipped (decided 2026-11-21: the counts are needed), so the dashboard only opens once the helper answers. There is no activity consent step any more (removed 2026-11-21).
- Settings (`settings.rs`, gear icon) is a compact list in the dashboard's bubble style, deliberately not a copy of the onboarding tiles:
  - an account row (name, "Hack Club", "Log out"; logging out unlinks the device on the server first, shows "Logging out" meanwhile and an error if that fails, then returns to the login step)
  - one row per agent (small logo, name, Connect/Disconnect)
  - an "Input helper" row (Linux Wayland, or wherever it's installed): Running / Not running, "Restart" and "Uninstall" (whenever installed), or "Set up" when it doesn't answer; each runs `pkexec … input-helper install|restart|uninstall` (`input_helper::run_as_admin`). Uninstalling on Wayland brings the required popup back.
  - a server row: a text field with the server address, "Save" (enabled once changed) and "Reset" back to production (only when a custom server is set). Only https, or http on localhost, is accepted. If the account doesn't belong to the new server, the window goes back to the login step.
- Top bar: gear on the dashboard, back arrow in settings and the debug screen, nothing during first start; a sun/moon icon top right switches light/dark and saves the choice.
- Debug screen (`debug.rs`): only with the client's `debug-screen` feature (default on; release builds from CI turn it off with `--no-default-features`, decided 2026-11-23). Ctrl+D pressed twice within 0.8 s opens it from any screen and closes it again (`keyboard::listen`, only key presses no widget used). It redraws every second from `Tracker::debug()` (`DebugInfo`, updated by the tracker each second): per followed session the current heartbeat's attention and credit multiplier so far (computed with `sisyphus_common::attention` from the last sent heartbeat's attention and the open minutes' readings), running and in-front state; the last input, the app class in front, how keys and mouse are told apart and this minute's counts, backends, watched folders, the prompt model's state and the last prompt's top act and task, heartbeats this run, the upload queue and the last upload; and the last heartbeat as JSON. Without a tracker in this window it says why.
- Dashboard:
  - Top half: recent sessions of all connected agents, newest first, as pink bubbles with logo, title, project folder and age. A running session shows "working" and a deeper pink bubble. Sessions are not clickable.
  - Bottom half: "Recorded today" and three figures. "Recorded today" and the "agent time" figure come from the server (`GET /api/app/today`: `tracked_ms`, `agent_ms`, for the local day; decided 2026-11-21), fetched with each 10 s refresh. "prompts" and "sessions" are placeholders (zero).
- Blocking work never runs on the UI thread:
  - `on_thread` runs it on its own thread and hands the result to a `Task`.
  - The session list reloads every 10 s through a `Subscription` whose stream is fed by a timer thread.
  - Background threads stop once their channel closes.
- Look (`style.rs`):
  - A light and a dark pink theme (`Theme::custom`), following the OS (`iced::system::theme` / `theme_changes`) unless the user picked one.
  - Font is Fredoka, embedded from `crates/client/assets/fonts/` (SIL OFL; keep `Fredoka-OFL.txt` next to the font files). The static files register as family "Fredoka Light".
  - Icons in `assets/icons/` are drawn for this project and tinted at runtime.
- UI direction from the user:
  - Simple, friendly and playful: pink, Fredoka, rounded bubbles, icons instead of text where possible, no unneeded text.
  - Avoid the generic "AI-generated" look the user rejected: status pills and badges, "·"-separated detail lines, outlines around list items, coloured status dots, monospace "dashboard" styling, default blue-grey themes.

### Testing the server

- Run a throwaway Postgres in a temporary directory: `initdb -D <dir> -A trust -U sisyphus`, then `pg_ctl -D <dir> -o "-p 54329 -k '' -c listen_addresses=localhost" start`, then create the database with `psql`. `-k ''` disables the Unix socket, whose path is too long inside the scratchpad.
- Start the server with `DATABASE_URL=postgres://sisyphus@localhost:54329/sisyphus` and a free `BIND_ADDR`.
- Signed test batches can be produced with `sisyphus_common::chain::ChainWriter` (for example from a temporary example program that isn't committed).
- Screenshot the page with `chromium --headless=new --screenshot --virtual-time-budget=4000`. Add `--force-dark-mode` for dark mode.
- Login can be tested end to end without real Hack Club credentials:
  - Run a small fake OIDC provider on `127.0.0.1` (discovery, authorize, token with PKCE check, JWKS; RS256-signed ID tokens, e.g. Python with the `cryptography` library), and point `HACKCLUB_AUTH_URL` at it. Give it a fresh key id per run, or the server keeps its cached key.
  - Drive the website flow with `curl -c/-b` cookie jars.
  - Also test with an `https://` `PUBLIC_URL` (the server can still listen on plain http). Cookie names differ there (`__Host-` / `__Secure-` prefixes), and curl won't send `Secure` cookies over http, so pass them back with `-H "Cookie: …"`.
  - Drive the app flow by putting a stand-in `xdg-open` on `PATH` that follows the redirects with curl.
  - Keep these helpers in the scratchpad, not the repo.
- Start helper processes with a PID file and stop them by PID. `pkill -f <pattern>` also kills the shell whose command line contains the pattern.
- Stop everything afterwards: `pkill -x sisyphus-server`, `pg_ctl -D <dir> stop`.

### Testing the review page

- Signed synthetic users (an honest person, an autotyper, a jiggler) can be made with a temporary example in `crates/common/examples/` using `ChainWriter` (not committed), uploaded to a local server, and then marked as received live (`UPDATE heartbeats SET received_ms = claimed_ms + 20000, stale = false, offline = false`, delete `session_minutes`, `minute_languages`, `minute_files` and restart: `backfill` credits them again), since anything an hour old is stale.
- A reviewer session without Hack Club: insert a `users` row with a Slack ID listed in `REVIEWERS` and a `sessions` row whose `token_hash` is the SHA-256 of a token, then send `Cookie: sisyphus_session=<token>` (on http).

### Testing the window

- Agents can't click in the window on Wayland. Verify by launching it and taking a screenshot (on Hyprland: `hyprctl clients -j` for the window geometry, then `grim -g`), and ask the user to click-test.
- Launch it with `XDG_CONFIG_HOME` pointing at a temporary directory, so the user's real config isn't touched. Write a config there to reach a given state (connected agents, `dark_mode`).
- Stop it with `pkill -x sisyphus-client`. `pkill -f` also matches the shell running the command. Closing the window doesn't stop it (it stays in the tray). The tray can be checked over D-Bus: `busctl --user get-property org.kde.StatusNotifierWatcher /StatusNotifierWatcher org.kde.StatusNotifierWatcher RegisteredStatusNotifierItems`, then `org.kde.StatusNotifierItem.ToolTip`, `com.canonical.dbusmenu.GetLayout` on `/MenuBar`, `Activate` (left click) and dbusmenu `Event <id> clicked` (menu items). On Hyprland a window is closed with `hyprctl dispatch "hl.dsp.window.close({window='address:<addr>'})"`; `dbus-run-session` gives a bus without a tray host.
- `watch` and `activity` never exit; wrap them in `timeout` when scripting.

### Testing activity

- Run `activity` with a temporary `XDG_CONFIG_HOME`. It prints key and mouse counts per second too.
- X11 and Windows key/mouse inference: `xdotool type --delay 120 "hello world"` should give 11 keys; under Wine type into the focused `wine notepad` without `--window` (Wine ignores synthetic events sent to a window).
- Input helper: don't run `serve` on the real devices (the user is in the `input` group; that would count their live typing). Run it in `bwrap --dev-bind / / --tmpfs /run --dir /run/sisyphus-input --tmpfs /dev/input --bind <fifo> /dev/input/event0` and write `struct input_event`s (`struct.pack("qqHHi", 0, 0, type, code, value)`) into the FIFO; a Python client on the socket sees the counts. Check the unit with `systemd-analyze verify` and `systemd-analyze security --offline=true`. Installing needs the user's password: ask them to click "Set up".
- Hyprland: as is. wlr-foreign-toplevel: unset `HYPRLAND_INSTANCE_SIGNATURE` (Hyprland supports the protocol too).
- X11: `Xvfb :77`, `openbox` and `xterm` on `DISPLAY=:77`, with `WAYLAND_DISPLAY` unset and `XDG_SESSION_TYPE=x11`; `xdotool mousemove` resets the idle time.
- Windows: `cargo build --target x86_64-pc-windows-gnu`, then run the `.exe` under `wine` with its own `WINEPREFIX` in the scratchpad, on the Xvfb display (`wine notepad` gives a focused window without touching the user's screen).
- macOS: the workspace can't be checked for `aarch64-apple-darwin` here (ring and sqlite need Apple's SDK). Type-check and clippy `macos.rs` in a scratch crate that includes it with `#[path]` and stubs `FocusReading`, `FocusedApp`, `Process` and `agent_of`, using the workspace `Cargo.lock` and `--offline`.
- Not testable here: Sway, GNOME, KDE. Ask the user to test those.

### Testing the tracker

- Start a throwaway Postgres and server (see "Testing the server"). Link a device without a login: run `track` once with a temporary `XDG_CONFIG_HOME` to create `device.key`, derive its public key (e.g. Python `cryptography`), and insert a `users` row and a `devices` row (`next_seq` 0, `prev` 64 zeros, `user_id`) with `psql`.
- Config: `connected_agents`, `server_url` pointing at the local server and a matching `account`.
- Run `timeout 400 sisyphus-client track`; the agent session you are working in is itself a running session, so your own edits show up as `agent_edits` and `working_dir`. Check the stored rows in `heartbeats`.

### Heartbeat design (decided 2026-11-20; the client records and uploads it, the server credits it, see "Crediting")

The full JSON scheme and the field meanings are in `REQUIREMENTS.md` ("Heartbeat scheme"). The essentials:

- **Per session.** Time is counted and calculated per agent session. Each session sends its own heartbeat each minute while its agent is running, or while it isn't running but its agent's window is in front (decided 2026-11-21).
- **What a heartbeat carries:**
  - the minute, the operating system family (`windows` / `macos` / `linux` / `other`, from the build target; decided 2026-11-22), agent, agent session id, the agent's session title, and the project folder name
  - whether the agent was running, `activity` (always true now), and `input_kinds`: how keys and mouse were told apart (`exact` / `inferred` / `unknown`)
  - six 10-second slots, each with whether there was input, key presses and mouse events counted, and the focused app class (`agent` / `terminal` / `editor` / `browser` / `other` / `testing` / `none` / `unknown`; all slots `unknown` without input when activity isn't allowed); plus how much input came from software (`injected`). v9 and v10 also had key presses and mouse events per second (`seconds`), dropped in v11
  - `tested_programs`: the project's own executables that were in front (focus `testing`), relative to the project folder
  - `running_programs`: the project's own executables running this minute (in front or not); a running program alone sends a heartbeat, earning nothing for now
  - prompt counts: submitted, typed chars, pasted chars, corrections, scripted; per prompt the local model's 22 scores (0–1000), the model's name and the second it was sent; interrupts
  - response counts: responses finished this minute and their visible characters (never text)
  - the client's version, the models that worked this minute, and the agent's tool calls by kind (count and call-to-result time) (v10)
  - per edited file, its path relative to the project folder, with non-whitespace characters added/removed in the working directory and what the agent's edit calls claim
- **Never from the client:** a 0–1 value, attention, the struggle value, or a session UUID.
- **Session identity.** The server builds its own session UUID from the machine id, the agent and the agent's session id. Each server-side session is named by the agent's session title.
- **Server computes** attention and the 0.0–1.0 multiplier per session. The client runs the same formulas only for display and to compare against the server.
- **Overlap.** If the multipliers of concurrent sessions for the same real minute add up to more than 1.0, each session gets its share of one minute (two sessions at 1.0 each get 30 s each).
- **Attention** is one value per heartbeat: more than 5 key presses in the agent's window in the minute make it 1.0 (mouse doesn't count there; in other apps, including testing, keys and mouse both count as activity); otherwise it drops from the previous heartbeat with a 2 min half-life if the agent's window held most of the minute, else 30 s. Without activity, a prompt is worth 1.0 and an interrupt 0.5, back to 0 the next minute. Only minutes where the agent ran earn credit. An agent working alone earns no credit; that time still counts as AI time. No mouse-jiggle rule yet (later).
- **Focused session.** A prompt submitted to a session means the user is focused on that session (decided 2026-11-21). The window match narrows candidates; the latest prompt picks among them.
- **No quick-switch farming.** Switching to the agent's window for ~5 s without real interaction gives little or no attention.
  - A slot's `focus` is the app that held focus for most of the slot.
  - Input (including alt-tab) only counts in slots where the agent's terminal or editor held focus, and there only key presses, more than 5 per heartbeat.
  - Attention never rises without input; watching without input only slows the decay when the agent's window held most of the minute.
- **Struggle value.** Each heartbeat has one, calculated on both the client and the server. Struggle does not affect credit for now; it is only kept.
  - Decided input: correction prompts.
  - Possible future signals, not decided (ask first): interrupts, rejected tool calls, re-prompting (near-identical prompts) and reverts (Claude Code rewinds, opencode reverts). Interrupts and rejected tool calls are already `AgentEvent`s.
- **No keylogging.** Input is detected through OS idle-time APIs and event counters, never which key:
  - Windows: `GetLastInputInfo`, keys vs mouse inferred
  - macOS: `CGEventSourceSecondsSinceLastEventType`, `CGEventSourceCounterForEventType`
  - Linux X11: XScreenSaver extension, keys vs mouse inferred
  - Linux Wayland: `ext-idle-notify` (Mutter's IdleMonitor on GNOME); key/mouse counts only through the input helper, which the user installs with their password after a popup that says exactly what it does (decided 2026-11-21). It reads `/dev/input` as its own unprivileged user and passes on counts only.
  - Activity is read without a consent step (decided 2026-11-21). Per compositor: see "Activity" above.
- **Edits must match.** While an agent runs, the client watches its working directory. The agent's edit calls and the actual file changes must match up, file by file; the server flags it when the agent claims more than actually changed.
- **Prompt classification.** A small local model classifies each prompt on the device (see "Prompt model"); only its per-category scores leave the machine.
  - Correction prompts ("no, you did this wrong, redo it") add to struggle; they don't reduce time, for now.
  - Pasted prompts earn less than typed ones.
  - Detecting AI-generated prompts is a low-priority idea.
- Not possible with this design: detecting deleting or rewriting inside a prompt draft.
- Open and for-later items (unmatched file changes, error-log pastes, struggle's effect on credit, tuning decay speeds, the mouse-jiggle rule, low values for other user actions without activity) are listed in `REQUIREMENTS.md`. Ask before deciding any of them.
