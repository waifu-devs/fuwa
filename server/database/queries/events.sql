-- name: CreateEvent :one
INSERT INTO events (event_id, event_type, scope, actor_id, timestamp, payload, metadata, sequence)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: GetEvent :one
SELECT * FROM events
WHERE event_id = ?;

-- name: GetEvents :many
SELECT * FROM events
WHERE scope = ?
  AND (event_type IN (sqlc.slice('event_types')) OR sqlc.slice('event_types') IS NULL)
  AND sequence >= ?
  AND sequence <= ?
ORDER BY sequence ASC
LIMIT ?;

-- name: GetEventsByScope :many
SELECT * FROM events
WHERE scope = ?
ORDER BY sequence DESC
LIMIT ? OFFSET ?;

-- name: GetLatestSequence :one
SELECT COALESCE(MAX(sequence), 0) as max_sequence
FROM events
WHERE scope = ?;

-- name: GetEventsByTypeAndScope :many
SELECT * FROM events
WHERE event_type = ? AND scope = ?
ORDER BY sequence DESC
LIMIT ?;