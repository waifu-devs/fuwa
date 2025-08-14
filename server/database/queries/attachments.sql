-- name: CreateAttachment :one
INSERT INTO attachments (attachment_id, message_id, filename, content_type, size, url)
VALUES (?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: GetAttachment :one
SELECT * FROM attachments
WHERE attachment_id = ?;

-- name: GetAttachmentsByMessageId :many
SELECT * FROM attachments
WHERE message_id = ?;

-- name: DeleteAttachment :exec
DELETE FROM attachments
WHERE attachment_id = ?;