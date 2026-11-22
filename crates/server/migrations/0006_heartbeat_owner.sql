-- Each heartbeat belongs to the account its device was linked to when it
-- was uploaded. Unlinking a device (from the app or the website) then keeps
-- its history and credited time with that account, and the device can be
-- linked again, even to another account, without taking that history along.
ALTER TABLE heartbeats ADD COLUMN user_id BIGINT REFERENCES users (id) ON DELETE SET NULL;

UPDATE heartbeats h SET user_id = d.user_id
FROM devices d
WHERE d.device_id = h.device_id AND d.user_id IS NOT NULL;

-- Heartbeats of devices already unlinked: their credited minutes still say
-- whose they were.
UPDATE heartbeats h SET user_id = m.user_id
FROM session_minutes m
WHERE h.user_id IS NULL AND m.device_id = h.device_id AND m.seq = h.seq;

CREATE INDEX heartbeats_user_idx ON heartbeats (user_id, received_ms DESC);
