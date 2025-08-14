-- name: CreateMessage :one
INSERT INTO messages (message_id, channel_id, author_id, content, created_at, updated_at, reply_to_id)
VALUES (?, ?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: GetMessage :one
SELECT * FROM messages
WHERE message_id = ?;

-- name: GetMessages :many
SELECT * FROM messages
WHERE channel_id = ?
  AND (created_at < ? OR ? = 0)
  AND (created_at > ? OR ? = 0)
ORDER BY created_at DESC
LIMIT ?;

-- name: UpdateMessage :one
UPDATE messages
SET content = ?, updated_at = ?
WHERE message_id = ?
RETURNING *;

-- name: DeleteMessage :exec
DELETE FROM messages
WHERE message_id = ?;

-- name: GetMessagesByChannelId :many
SELECT * FROM messages
WHERE channel_id = ?
ORDER BY created_at DESC
LIMIT ? OFFSET ?;