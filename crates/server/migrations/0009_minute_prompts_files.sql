-- More per-minute stats (formula 6, decided 2026-11-23): prompts sent, and
-- the files that changed with their characters added and removed in the
-- working directory (paths relative to the project folder, as uploaded).
ALTER TABLE session_minutes ADD COLUMN prompts INTEGER NOT NULL DEFAULT 0;

CREATE TABLE minute_files (
    session_id TEXT   NOT NULL,
    minute_ms  BIGINT NOT NULL,
    path       TEXT   NOT NULL,
    chars      BIGINT NOT NULL,
    PRIMARY KEY (session_id, minute_ms, path),
    FOREIGN KEY (session_id, minute_ms)
        REFERENCES session_minutes (session_id, minute_ms) ON DELETE CASCADE
);
