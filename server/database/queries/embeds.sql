-- name: CreateEmbed :one
INSERT INTO embeds (embed_id, message_id, title, description, url, color, thumbnail_url, image_url)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: GetEmbed :one
SELECT * FROM embeds
WHERE embed_id = ?;

-- name: GetEmbedsByMessageId :many
SELECT * FROM embeds
WHERE message_id = ?;

-- name: DeleteEmbed :exec
DELETE FROM embeds
WHERE embed_id = ?;