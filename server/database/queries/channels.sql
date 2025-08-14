-- name: CreateChannel :one
INSERT INTO channels (channel_id, name, type, server_id, parent_id, metadata, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: GetChannel :one
SELECT * FROM channels
WHERE channel_id = ?;

-- name: ListChannels :many
SELECT * FROM channels
WHERE (server_id = ? OR ? = '')
  AND (parent_id = ? OR ? = '')
ORDER BY created_at DESC
LIMIT ? OFFSET ?;

-- name: UpdateChannel :one
UPDATE channels 
SET name = ?, metadata = ?, updated_at = ?
WHERE channel_id = ?
RETURNING *;

-- name: DeleteChannel :exec
DELETE FROM channels
WHERE channel_id = ?;

-- name: ListChannelsByServerId :many
SELECT * FROM channels
WHERE server_id = ?
ORDER BY created_at DESC;
