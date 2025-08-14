-- name: CreateEmbedField :one
INSERT INTO embed_fields (field_id, embed_id, name, value, inline)
VALUES (?, ?, ?, ?, ?)
RETURNING *;

-- name: GetEmbedFieldsByEmbedId :many
SELECT * FROM embed_fields
WHERE embed_id = ?;

-- name: DeleteEmbedField :exec
DELETE FROM embed_fields
WHERE field_id = ?;