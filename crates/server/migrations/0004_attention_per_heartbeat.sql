-- Attention is one value per heartbeat (formula 3); nothing per slot
-- carries over between minutes anymore.
ALTER TABLE session_minutes DROP COLUMN last_slot_agent;
