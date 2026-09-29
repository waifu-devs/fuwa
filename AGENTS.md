# AGENTS.md: working on fuwa

## Layout

- `proto/fuwa/v1/`: the protocol (gRPC). `buf lint` must pass; the Rust code is
  generated from it at build time (`server/build.rs`, protoc is vendored).
- `server/`: the Rust server (`fuwa` binary, `fuwa_server` library).
  - `app.rs`: shared state, the HTTP router (gRPC, gRPC-Web, CORS, health), serving.
  - `api/`: one file per gRPC service, all implemented on `Api`.
  - `node.rs`: the instance database (`node.db`): accounts, sessions, meta.
  - `servers.rs`: community servers, one Turso file each under `servers/`, plus the
    in-memory index of servers and memberships. Every change goes through
    `ServerDb::write`, which appends events to the server's log in the same
    transaction and publishes them to the `Hub` after commit.
  - `db.rs`: Turso helpers: opening, `user_version` migrations, transactions.
  - `config.rs`: `FUWA_*` environment variables.
  - `telemetry.rs`: the anonymous usage signal (schema `fuwa.signal.v1`).
  - `migrations/node`, `migrations/server`: SQL applied in order, tracked in
    `PRAGMA user_version`. Never edit a migration that has shipped; add a new file
    and list it in `MIGRATIONS`.
  - `tests/api.rs`: end-to-end tests against a running instance.

## Rules

- Every change to a community server is a `ServerDb::write` that pushes at least
  one event payload and keeps the `usage` counters in step.
- Timestamps are unix milliseconds in the database, `google.protobuf.Timestamp` on
  the wire. Ids are ULIDs (`id::new_id`).
- Limits are unlimited unless configured. Never hardcode a usage cap.
- The usage signal must stay anonymous: no content, names or ids of people or
  servers. The end-to-end test checks this.
- Before pushing: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, and `buf lint`.
