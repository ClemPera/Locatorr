CREATE TABLE IF NOT EXISTS rendezvous_rooms (
    id TEXT PRIMARY KEY,
    expires_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS rendezvous_pubkeys (
    room_id TEXT NOT NULL REFERENCES rendezvous_rooms(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('initiator', 'responder')),
    pubkey BYTEA NOT NULL,
    PRIMARY KEY (room_id, role)
);

CREATE TABLE IF NOT EXISTS rendezvous_bundles (
    room_id TEXT NOT NULL REFERENCES rendezvous_rooms(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('initiator', 'responder')),
    bundle BYTEA NOT NULL,
    PRIMARY KEY (room_id, role)
);

CREATE TABLE IF NOT EXISTS inbox (
    id BIGSERIAL PRIMARY KEY,
    user_id TEXT NOT NULL,
    payload BYTEA NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_inbox_user_id ON inbox(user_id);
