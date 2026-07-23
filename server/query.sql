-- name: InsertRoom :one
INSERT INTO rendezvous_rooms (id, expires_at)
VALUES ($1, $2)
RETURNING id, expires_at;

-- name: GetRoom :one
SELECT id, expires_at FROM rendezvous_rooms
WHERE id = $1 AND expires_at > NOW();

-- name: UpsertPubkey :exec
INSERT INTO rendezvous_pubkeys (room_id, role, pubkey)
VALUES ($1, $2, $3)
ON CONFLICT (room_id, role) DO UPDATE SET pubkey = EXCLUDED.pubkey;

-- name: GetPubkeys :many
SELECT role, pubkey FROM rendezvous_pubkeys
WHERE room_id = $1;

-- name: UpsertBundle :exec
INSERT INTO rendezvous_bundles (room_id, role, bundle)
VALUES ($1, $2, $3)
ON CONFLICT (room_id, role) DO UPDATE SET bundle = EXCLUDED.bundle;

-- name: GetBundles :many
SELECT role, bundle FROM rendezvous_bundles
WHERE room_id = $1;

-- name: InsertInboxMessage :exec
INSERT INTO inbox (user_id, payload)
VALUES ($1, $2);

-- name: GetAndDeleteInbox :many
DELETE FROM inbox
WHERE user_id = $1
RETURNING id, user_id, payload, created_at;