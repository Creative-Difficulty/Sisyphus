-- Devices and the heartbeats they uploaded.
--
-- A device is identified by its ed25519 public key (hex). Its row holds the
-- end of its hash chain, which every new upload must continue.

CREATE TABLE devices (
    device_id     TEXT PRIMARY KEY,
    -- Chain tip: the sequence number and previous hash the next heartbeat
    -- must have.
    next_seq      BIGINT NOT NULL,
    prev          TEXT   NOT NULL,
    first_seen_ms BIGINT NOT NULL,
    last_seen_ms  BIGINT NOT NULL
);

CREATE TABLE heartbeats (
    device_id   TEXT   NOT NULL REFERENCES devices (device_id),
    seq         BIGINT NOT NULL,
    hash        TEXT   NOT NULL,
    prev        TEXT   NOT NULL,
    sig         TEXT   NOT NULL,
    -- When the client says it happened (untrusted) and when the server got it.
    claimed_ms  BIGINT NOT NULL,
    received_ms BIGINT NOT NULL,
    -- The signed heartbeat body as sent. Kept as JSON so the heartbeat format
    -- can change (v2) without rewriting this table.
    body        JSONB  NOT NULL,
    PRIMARY KEY (device_id, seq)
);

CREATE INDEX heartbeats_received_idx ON heartbeats (received_ms DESC);
