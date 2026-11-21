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
  - Only counts, timings, coarse app classes, paths of edited files relative to the project folder, and the agent's session titles may leave the machine. Paths and titles are visible only to the user and reviewers, never on public profiles.
  - AI chat history and prompts stay local. `AgentEvent`s carry full prompt and response text in memory on the client; anything that leaves the machine must be reduced to what the heartbeat scheme allows.
- Never write to, commit to or modify a user's repository. Everything runs in the background.
- Never credit more time than was actually tracked. Multipliers from 0.0 to 1.0 are applied server-side.
- Never trust the client clock for anything audited. The server timestamps.
- No new dependencies without a one-line justification and the user's approval. Put new ones in `[workspace.dependencies]`.
- Keep Windows, macOS and Linux working, and keep the client light (low-spec machines).
- Never commit unless the user asks. After every commit, push (`git push origin main`). Before pushing, `git fetch` and check nobody else pushed in the meantime. Keep commits separate by topic.
- Before committing: `cargo build`, `cargo test --workspace` and `cargo clippy` clean, `cargo fmt` applied to the crates you touched.
- Never build, pull or run Docker/Podman images. The `Dockerfile` is for deployment only; the user dislikes Docker locally.

## Commands

```sh
cargo build                                  # whole workspace
cargo build --release -p sisyphus-client     # release client
cargo test --workspace                       # all tests
cargo test -p sisyphus-common --test chain   # one integration test file
cargo test -p sisyphus-client opencode::session::tests::abort_is_an_interrupt  # one test, by path filter
cargo clippy --workspace --all-targets
cargo fmt -p sisyphus-client                 # crates/common is not rustfmt-clean yet; don't reformat it unasked

cargo run -p sisyphus-client                                  # open the window
cargo run -p sisyphus-client -- sessions                      # list sessions of all agents, with running state
cargo run -p sisyphus-client -- watch [SESSION_ID] [--replay] # print a session's events live; never exits on its own

cargo run -p sisyphus-server                 # needs DATABASE_URL (from the environment or .env, see .env.example)
```

No `rust-toolchain` file or CI config exists. Edition 2024, resolver 3 (let-chains are used). Dependencies are declared in `[workspace.dependencies]` in the root `Cargo.toml` and referenced with `.workspace = true`.

## Architecture

Three crates in `crates/`:

- **`sisyphus-common`**: types shared by client and server.
  - Data flow (see `lib.rs` docs): collectors produce `Heartbeat`s → the client signs them into a per-device hash chain and queues them offline → batches are uploaded (`api.rs`) → the server verifies the batch continues the stored `ChainTip`, stamps its own receive time, and flags heartbeats received more than 5 min after their claimed time as offline.
  - `chain.rs`: sequence number, previous hash, SHA-256 with domain tag `sisyphus/heartbeat/v1`, ed25519 signature.
  - Heartbeats are hashed and signed, so they must serialize deterministically: no floats, no maps. Changing the hashed format means bumping the domain tag.
  - The tamper-detection tests in `crates/common/tests/` must keep passing.
  - `heartbeat.rs` is still the v1 shape (WakaTime-style gaps, free-form `app` string). The final v2 scheme is in `REQUIREMENTS.md` ("Heartbeat scheme"); only switch to it when the user asks.
- **`sisyphus-client`**: a library (`lib.rs`: `agents`, `config`, `device`, `gui`, `login`) plus a binary (`main.rs`).
  - Run without arguments, the binary opens the iced window; `sessions` and `watch` are CLI commands over the same integrations.
  - `device.rs`: this install's ed25519 key, created once from OS randomness and stored as `device.key` (hex) next to the config, mode 0600 on Unix. It never leaves the machine; it signs heartbeats and proves the device when linking.
  - `login.rs`: logs the app in through the server (see "Accounts" below). Blocking; the window runs it on a worker thread. It refuses non-https servers except on localhost.
  - It does not yet build heartbeats, use the chain or upload anything.
- **`sisyphus-server`**: axum + Postgres (sqlx).
  - Configured from the environment or `.env` (`.env` is git-ignored; `.env.example` lists every key): `DATABASE_URL`, `BIND_ADDR` (default `0.0.0.0:8080`), `PUBLIC_URL` (login redirect and whether cookies are `Secure`), `HACKCLUB_CLIENT_ID` / `HACKCLUB_CLIENT_SECRET` (login is disabled without them), optional `HACKCLUB_AUTH_URL`.
  - Migrations in `crates/server/migrations/` are embedded (`sqlx::migrate!`) and run on start.
    - `devices`: device key, chain tip, first/last seen, owning user.
    - `heartbeats`: device, seq, hash, prev, sig, claimed and received time, and the heartbeat body as `JSONB` (so the v2 format won't need a table rewrite).
    - `users`, `sessions`, `login_attempts`, `app_codes`: see "Accounts" below.
  - Queries are runtime-checked (`sqlx::query`, not the `query!` macros), so building needs no database.
  - `POST /api/upload` (`upload.rs`): an `UploadBatch` from `sisyphus-common::api`.
    - Only devices linked to an account may upload (403 otherwise).
    - The device's row is locked and its chain tip checked with `verify_batch`.
    - On success the heartbeats are stored with the server's receive time and an `UploadAck` with the new tip comes back.
    - 409 with `OutOfSync` (resend from the expected tip) or 400 with `Empty`/`Invalid`.
  - `GET /api/dashboard` (`dashboard.rs`): the logged-in user's devices, total heartbeats and 50 latest heartbeats, each flagged offline if received more than 5 min after its claimed time (backend only; the page doesn't show it). 401 without a session.
  - `GET /health` returns `ok`.
  - The web page (`crates/server/web/`: plain HTML, CSS and JS, no framework) and the Fredoka fonts are embedded in the binary (`web.rs`).
    - It matches the client: pink light/dark theme (OS-following, sun/moon switch saved in `localStorage`), Fredoka, pink bubbles, and a placeholder "Recorded today".
    - Logged out it shows a "Log in with Hack Club" button; logged in, the dashboard plus the name and "Log out" in the header.
    - It refreshes every 10 s. Data is inserted with `textContent` only, never as HTML.
    - Every response carries security headers (`main.rs`): a strict Content-Security-Policy (no inline scripts or styles, which is why `theme.js` is its own file), `X-Frame-Options: DENY`, `nosniff`, `Referrer-Policy: no-referrer`, and HSTS on https.
  - `Dockerfile` at the repo root builds only the server (`rust:1.95-slim-trixie`, then `debian:trixie-slim`), with BuildKit cache mounts for crates and `target/` so rebuilds are incremental. Only ever build it on the production server; see the Docker rule above.
  - It does not yet build session UUIDs, compute attention or credit, or know the v2 heartbeat.

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
  2. After the same Hack Club login, the callback redirects the browser to `http://127.0.0.1:<port>/done?code=…`. The code is one-time, 2 minutes, stored hashed.
  3. The app posts `AppTokenRequest` to `/auth/app/token`: the code, the verifier, and a device signature over `device_link_message(code)` (`sisyphus-common::api`).
  4. The server checks all three and links the device to the user. A device linked to another account can't be taken over (409).
  - The app gets no bearer token: uploads are already signed by the device key, and the server accepts uploads only from linked devices.
- Discovery endpoints must be https; plain http is only accepted when `HACKCLUB_AUTH_URL` is itself `http://localhost` / `http://127.0.0.1` (local testing).
- Not built yet: unlinking a device from the website, and rate limiting. "Log out" in the app only forgets the account locally; the device stays linked on the server.

### Agent integrations (`crates/client/src/agents/`)

`mod.rs` defines the plugin interface:
- `AgentIntegration` discovers sessions (`get_instances`), and has `install()` / `is_installed()`. Its signature was written by hand; extend it rather than restructuring it.
- `AgentInstance` is one session: title, uuid (the agent's session id), project path, last activity, `is_running()`, `read_events()`, and `attach(AttachMode)`.
- `attach` returns an `Attachment`, which wraps a worker thread and an mpsc channel of `AgentEvent`s; dropping it stops the worker.
- `AgentEventKind`:
  - `Prompt { text, pasted_chars }`, `Command`, `Response`, `Thinking`
  - `ToolCall` and `ToolResult` (outcome: success / error / rejected by the user)
  - `Interrupt`, `TurnEnd`, `Compacted`, `TitleChanged`
- `Integration` is an enum over all integrations (`Integration::ALL`), with `id()`, `name()`, `instances()`, `instance(id)`, `is_installed()` and `install()`, so callers can treat every agent the same. `id()` values are stored in the config; never change an existing one.
- `is_running()` means the agent is mid-turn: the newest event isn't a turn end or interrupt (Claude Code), or the newest prompt/reply row leaves the turn open (opencode). Both also require activity in the last 15 min, so a crashed session doesn't stay "running".
- `install()` does nothing yet beyond checking the agent is present, because both integrations only read files. It is the hook for integrations that will need something set up in the agent.

Neither integration installs hooks. Both read the agent's own on-disk storage, whose format is undocumented: skip unknown lines and fields rather than erroring.

- **`claude_code/`** reads the append-only JSONL transcripts at `~/.claude/projects/<escaped cwd>/<session uuid>.jsonl` (or `$CLAUDE_CONFIG_DIR`; falls back to `USERPROFILE` on Windows).
  - `transcript.rs` maps lines to events. It filters `isMeta`, `isSidechain` and compaction-summary lines, and lines whose `origin.kind` is not human. Interrupts are `[Request interrupted by user…]` text. Rejected tool calls are recognised by `toolUseResult`.
  - Sessions are listed by scanning only the head and tail of each file, because transcripts reach hundreds of MB and title lines are re-appended.
  - Live following uses `tail.rs`, a polling tail. Claude Code flushes its transcript about a second late.
- **`opencode/`** reads opencode v2's SQLite database (`$XDG_DATA_HOME/opencode/opencode.db`), read-only, using the `session_v2` and `session_message` tables.
  - The v1 `session`/`message`/`part` tables are legacy (already migrated) and are not read.
  - Assistant rows are rewritten in place while streaming. Attachments therefore poll by `time_updated` (`>=` watermark), and `session.rs` deduplicates by remembering which events it already emitted.
  - The database and prompt-history paths only use `XDG_*` / `HOME`, so opencode isn't found on Windows yet.
- `Prompt.pasted_chars` comes from each agent's prompt history, not the session itself:
  - `claude_code/history.rs` reads `~/.claude/history.jsonl`, matched by session id and timestamp, with a cross-session fallback for resumed sessions.
  - `opencode/history.rs` reads `$XDG_STATE_HOME/opencode/prompt-history.jsonl`, matched by the expanded prompt text. Its placeholder offsets may be UTF-16, so placeholders are found by text.
  - Only large pastes are recorded there, so small pastes count as typed. `None` means the prompt wasn't in the history: queued prompts, or opencode prompts older than its ~50-entry history.
  - Only lengths (and a hash, for opencode) are kept, never pasted text.

To add an agent:
1. Add a submodule implementing both traits.
2. Add an `Integration` variant and its arms in every `Integration` helper, including a new `id()`.
3. Add its logo SVG to `crates/client/assets/` and an arm in `gui::style::logo`.

### Local config (`config.rs`)

JSON in the OS config directory:
- Linux: `$XDG_CONFIG_HOME/sisyphus/config.json`
- macOS: `~/Library/Application Support/sisyphus/`
- Windows: `%APPDATA%\sisyphus\`

Fields:
- `connected_agents`: a list of `Integration::id()`s.
- `dark_mode`: the window's light/dark choice; absent means follow the OS.
- `server_url`: the Sisyphus server; absent means `http://localhost:8080`.
- `account`: `{ "name": … }` once logged in, absent when logged out.

`device.key` (the device's private key) lives next to it.

Unknown fields are ignored. Saving replaces the file atomically.

### Window (`crates/client/src/gui/`, iced 0.14)

- `mod.rs`: the app state, `Message`s and screens. `resume()` picks the screen from the config: not logged in → login step; no agents → connect step; otherwise the dashboard.
- First start (`onboarding.rs`), never the settings screen:
  1. Login step: name, one line, "Log in with Hack Club". After logging in it shows the name and "Continue". Login is required.
  2. Connect step: agent tiles (logo, name, Connect/Disconnect) and "Continue", enabled once an agent is connected.
- Settings (`settings.rs`, gear icon) is a compact list in the dashboard's bubble style, deliberately not a copy of the onboarding tiles:
  - an account row (name, "Hack Club", "Log out"; logging out returns to the login step)
  - one row per agent (small logo, name, Connect/Disconnect)
- Top bar: gear on the dashboard, back arrow in settings, nothing during first start; a sun/moon icon top right switches light/dark and saves the choice.
- Dashboard:
  - Top half: recent sessions of all connected agents, newest first, as pink bubbles with logo, title, project folder and age. A running session shows "working" and a deeper pink bubble. Sessions are not clickable.
  - Bottom half: "Recorded today" and three figures. These are placeholders (zero) until time is recorded; don't compute anything there unless asked.
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
  - Drive the app flow by putting a stand-in `xdg-open` on `PATH` that follows the redirects with curl.
  - Keep these helpers in the scratchpad, not the repo.
- Start helper processes with a PID file and stop them by PID. `pkill -f <pattern>` also kills the shell whose command line contains the pattern.
- Stop everything afterwards: `pkill -x sisyphus-server`, `pg_ctl -D <dir> stop`.

### Testing the window

- Agents can't click in the window on Wayland. Verify by launching it and taking a screenshot (on Hyprland: `hyprctl clients -j` for the window geometry, then `grim -g`), and ask the user to click-test.
- Launch it with `XDG_CONFIG_HOME` pointing at a temporary directory, so the user's real config isn't touched. Write a config there to reach a given state (connected agents, `dark_mode`).
- Stop it with `pkill -x sisyphus-client`. `pkill -f` also matches the shell running the command.
- `watch` never exits; wrap it in `timeout` when scripting.

### Heartbeat design (decided 2026-11-20, not implemented yet)

The full JSON scheme and the field meanings are in `REQUIREMENTS.md` ("Heartbeat scheme"). The essentials:

- **Per session.** Time is counted and calculated per agent session. Each session sends its own heartbeat each minute while its agent is running or the user is working on its prompt.
- **What a heartbeat carries:**
  - the minute, agent, agent session id, the agent's session title, and the project folder name
  - whether the agent was running
  - six 10-second slots, each with whether there was input and the focused app class (`agent` / `terminal` / `editor` / `browser` / `other` / `none`)
  - prompt counts: submitted, typed chars, pasted chars, corrections
  - per edited file, its path relative to the project folder, with non-whitespace characters added/removed in the working directory and what the agent's edit calls claim
- **Never from the client:** a 0–1 value, attention, the struggle value, or a session UUID.
- **Session identity.** The server builds its own session UUID from the machine id, the agent and the agent's session id. Each server-side session is named by the agent's session title.
- **Server computes** attention and the 0.0–1.0 multiplier per session. The client runs the same formulas only for display and to compare against the server.
- **Overlap.** If the multipliers of concurrent sessions for the same real minute add up to more than 1.0, each session gets its share of one minute (two sessions at 1.0 each get 30 s each).
- **Attention** resets to 1.0 on input. It drops slowly while the agent's window stays focused and quickly in other apps. An agent working alone earns no credit; that time still counts as AI time.
- **No quick-switch farming.** Switching to the agent's window for ~5 s without real interaction gives little or no attention.
  - A slot's `focus` is the app that held focus for most of the slot.
  - Input (including alt-tab) only counts in slots where the agent's terminal or editor held focus.
  - Watching without input only counts after ~20 s (two slots) of continuous focus. Thresholds get tuned with real data.
- **Struggle value.** Each heartbeat has one, calculated on both the client and the server. Struggle does not affect credit for now; it is only kept.
  - Decided input: correction prompts.
  - Possible future signals, not decided (ask first): interrupts, rejected tool calls, re-prompting (near-identical prompts) and reverts (Claude Code rewinds, opencode reverts). Interrupts and rejected tool calls are already `AgentEvent`s.
- **No keylogging and no root.** Input is detected only through OS idle-time APIs:
  - Windows: `GetLastInputInfo`
  - macOS: `CGEventSourceSecondsSinceLastEventType`
  - Linux X11: XScreenSaver extension
  - Linux Wayland: `ext-idle-notify`. Reading the focused app needs the user's permission: a popup asks for it and explains how the data is used. How it is read per compositor is still open.
- **Edits must match.** While an agent runs, the client watches its working directory. The agent's edit calls and the actual file changes must match up, file by file; the server flags it when the agent claims more than actually changed.
- **Prompt classification.** A really small local embedding model classifies each prompt on the device. The model is deliberately not chosen yet: the architecture comes first. Only the label leaves the machine.
  - Correction prompts ("no, you did this wrong, redo it") add to struggle; they don't reduce time, for now.
  - Pasted prompts earn less than typed ones.
  - Detecting AI-generated prompts is a low-priority idea.
- Not possible with this design: detecting deleting or rewriting inside a prompt draft.
- Open and for-later items (unmatched file changes, Wayland focus, error-log pastes, struggle's effect on credit, the embedding model, attention decay speeds, credit for reviewing after a turn) are listed in `REQUIREMENTS.md`. Ask before deciding any of them.
