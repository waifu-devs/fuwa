-- name: GetConfig :one
SELECT * FROM config_values
WHERE scope = ? AND key = ?;

-- name: GetConfigs :many
SELECT * FROM config_values
WHERE scope = ?
  AND (key IN (sqlc.slice('keys')) OR sqlc.slice('keys') IS NULL);

-- name: SetConfig :one
INSERT INTO config_values (scope, key, value, type, is_sensitive, constraints, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(scope, key) DO UPDATE SET
  value = excluded.value,
  type = excluded.type,
  is_sensitive = excluded.is_sensitive,
  constraints = excluded.constraints,
  updated_at = excluded.updated_at
RETURNING *;

-- name: DeleteConfig :one
DELETE FROM config_values
WHERE scope = ? AND key = ?
RETURNING *;

-- name: ListConfigKeys :many
SELECT * FROM config_values
WHERE scope = ?
  AND (key LIKE ? || '%' OR ? = '');