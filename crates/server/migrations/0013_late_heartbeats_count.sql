-- Heartbeats that arrived late earn credit again, flagged `offline`
-- (decided 2026-11-25). `stale` now only marks heartbeats dated before
-- their device was linked (recorded while logged out). Ones flagged only
-- for arriving more than an hour late are cleared; credit is rebuilt
-- because the formula changed. A device relinked since keeps only the
-- flags that its current link explains.
UPDATE heartbeats h SET stale = false
WHERE stale AND NOT EXISTS (
    SELECT 1 FROM devices d
    WHERE d.device_id = h.device_id
      AND h.claimed_ms < d.linked_ms
      AND h.received_ms >= d.linked_ms
);
