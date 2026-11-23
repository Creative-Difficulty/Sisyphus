-- What the dashboard's stats break credited time down by, per credited
-- minute. Derived data like the rest of `session_minutes`: rebuilt from
-- the heartbeats when `FORMULA` changes.

-- The heartbeat's operating system family ('' before heartbeat v7), and
-- the top task of the session's latest scored prompt so far ('' before
-- the session's first scored prompt).
ALTER TABLE session_minutes ADD COLUMN os TEXT NOT NULL DEFAULT '';
ALTER TABLE session_minutes ADD COLUMN task TEXT NOT NULL DEFAULT '';

-- The minute's file changes per language: non-whitespace characters added
-- and removed in the working directory. A minute's credit is split across
-- its languages in proportion; a minute without rows changed no files.
CREATE TABLE minute_languages (
    session_id TEXT   NOT NULL,
    minute_ms  BIGINT NOT NULL,
    language   TEXT   NOT NULL,
    chars      BIGINT NOT NULL,
    PRIMARY KEY (session_id, minute_ms, language),
    FOREIGN KEY (session_id, minute_ms)
        REFERENCES session_minutes (session_id, minute_ms) ON DELETE CASCADE
);
