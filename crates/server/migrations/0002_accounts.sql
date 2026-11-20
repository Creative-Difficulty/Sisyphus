-- Accounts via Hack Club Auth, web sessions, and linking devices to users.
--
-- No Hack Club tokens are stored: the server only needs to know who logged
-- in. Session tokens and one-time codes are stored as SHA-256 hashes, so a
-- database leak doesn't hand out working logins.

CREATE TABLE users (
    id                  BIGSERIAL PRIMARY KEY,
    -- Hack Club Auth's stable user id (the OIDC `sub`, e.g. "ident!abc123").
    hackclub_id         TEXT    NOT NULL UNIQUE,
    name                TEXT,
    slack_id            TEXT,
    created_ms          BIGINT  NOT NULL,
    last_login_ms       BIGINT  NOT NULL
);

-- Browser sessions for the website.
CREATE TABLE sessions (
    token_hash TEXT   PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_ms BIGINT NOT NULL,
    expires_ms BIGINT NOT NULL
);
CREATE INDEX sessions_user_idx ON sessions (user_id);

-- Logins in progress: one row per redirect to Hack Club Auth, used once.
CREATE TABLE login_attempts (
    state_hash    TEXT   PRIMARY KEY,
    nonce         TEXT   NOT NULL,
    pkce_verifier TEXT   NOT NULL,
    -- Set when the login was started by the desktop app.
    app_port      INTEGER,
    app_challenge TEXT,
    app_device_id TEXT,
    expires_ms    BIGINT NOT NULL
);

-- One-time codes handed to the desktop app after a successful login.
CREATE TABLE app_codes (
    code_hash  TEXT   PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_id  TEXT   NOT NULL,
    challenge  TEXT   NOT NULL,
    expires_ms BIGINT NOT NULL
);

-- Devices belong to a user once linked. Uploads from unlinked devices are
-- refused. Rows from before accounts existed keep a NULL owner.
ALTER TABLE devices ADD COLUMN user_id BIGINT REFERENCES users (id) ON DELETE SET NULL;
CREATE INDEX devices_user_idx ON devices (user_id);
