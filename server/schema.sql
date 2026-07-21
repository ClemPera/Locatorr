-- name: CreateRoom exec
CREATE TABLE IF NOT EXISTS rendezvous_rooms (
    id text PRIMARY KEY,
    created_by text NOT NULL, -- TODO:fk to users
    expires_at TIMESTAMP NOT NULL
);
