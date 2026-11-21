-- Heartbeats received more than 5 minutes after their minute were recorded
-- offline. They still count; reviewers see the flag.
ALTER TABLE heartbeats ADD COLUMN offline BOOLEAN NOT NULL DEFAULT false;
UPDATE heartbeats SET offline = received_ms - claimed_ms > 300000;

-- The user's time zone (IANA name, e.g. "Europe/Berlin") for "today" and
-- "this week". NULL until set on the website.
ALTER TABLE users ADD COLUMN timezone TEXT;

-- The server's own sessions: one per device, agent and agent session id.
CREATE TABLE agent_sessions (
    -- UUID-formatted SHA-256 of device id, agent and agent session id.
    id               TEXT   PRIMARY KEY,
    user_id          BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_id        TEXT   NOT NULL REFERENCES devices (device_id),
    agent            TEXT   NOT NULL,
    agent_session_id TEXT   NOT NULL,
    -- The agent's latest session title and the project folder name.
    name             TEXT   NOT NULL,
    project          TEXT   NOT NULL,
    first_minute_ms  BIGINT NOT NULL,
    last_minute_ms   BIGINT NOT NULL
);
CREATE INDEX agent_sessions_user_idx ON agent_sessions (user_id);

-- One row per session and minute: attention and credit, derived from the
-- heartbeat. Derived data: it can be deleted and rebuilt from heartbeats
-- when the formula changes (`formula` says which one computed it).
CREATE TABLE session_minutes (
    session_id     TEXT    NOT NULL REFERENCES agent_sessions (id) ON DELETE CASCADE,
    minute_ms      BIGINT  NOT NULL,
    user_id        BIGINT  NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The heartbeat it came from.
    device_id      TEXT    NOT NULL,
    seq            BIGINT  NOT NULL,
    -- Attention after the minute's last slot, carried into the next minute,
    -- and whether that slot had the agent's window in front.
    attention_end  DOUBLE PRECISION NOT NULL,
    last_slot_agent BOOLEAN NOT NULL,
    -- 0.0 to 1.0, before sharing the minute with concurrent sessions.
    multiplier     DOUBLE PRECISION NOT NULL,
    -- Credited time after sharing; never more than 60000.
    credit_ms      BIGINT  NOT NULL,
    -- 60000 when the agent ran this minute (AI time, kept apart from credit).
    ai_ms          BIGINT  NOT NULL,
    offline        BOOLEAN NOT NULL,
    formula        INTEGER NOT NULL,
    PRIMARY KEY (session_id, minute_ms)
);
CREATE INDEX session_minutes_user_minute_idx ON session_minutes (user_id, minute_ms);
CREATE INDEX session_minutes_heartbeat_idx ON session_minutes (device_id, seq);
