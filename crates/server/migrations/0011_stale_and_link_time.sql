-- Heartbeats that earn no credit (decided 2026-11-23), kept so the chain
-- stays whole and reviewers see them:
-- - received more than an hour after their minute (real agent work needs
--   the network, so it isn't recorded offline for long)
-- - dated before their device was linked to the account it was uploaded to
--   (recorded while logged out)
ALTER TABLE heartbeats ADD COLUMN stale BOOLEAN NOT NULL DEFAULT false;
UPDATE heartbeats SET stale = received_ms - claimed_ms > 3600000;

-- When the device was last linked to its account. Devices linked before
-- this column existed get 0 (no lower bound).
ALTER TABLE devices ADD COLUMN linked_ms BIGINT NOT NULL DEFAULT 0;

-- The review tool reads a user's heartbeats by time.
CREATE INDEX heartbeats_user_claimed_idx ON heartbeats (user_id, claimed_ms);
