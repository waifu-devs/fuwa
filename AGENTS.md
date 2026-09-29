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
    Every database runs in Turso's concurrent-writer mode (MVCC): writes are
    `BEGIN CONCURRENT` transactions that run side by side and are retried
    when two touch the same row.
  - `config.rs`: `FUWA_*` environment variables: how the process starts, and
    the defaults for settings.
  - `settings.rs`: settings admins change from a client (`AdminService`), stored
    in node.db's `settings` table over the environment's defaults. Read them
    through `app.settings()`, never from `config`, so changes apply at once.
  - `telemetry.rs`: the anonymous usage signal (schema `fuwa.signal.v1`).
  - `migrations/node`, `migrations/server`: SQL applied in order, tracked in
    `PRAGMA user_version`. Never edit a migration that has shipped; add a new file
    and list it in `MIGRATIONS`.
  - `web.rs`: serves the embedded web app (feature `web`, from `web/dist`), with
    `index.html` for any path the API doesn't answer so deep links work.
  - `tests/api.rs`: end-to-end tests against a running instance; `tests/web.rs`
    covers the embedded app.
- `web/`: the web app (pnpm, Vite, React 19, TanStack Router, Tailwind 4,
  shadcn/ui and Animate UI copied from the waifu.dev site, Effect).
  - `src/gen/`: protobuf code from `pnpm generate`. Generated, committed, never
    edited by hand.
  - `src/fuwa/`: talking to instances. `sync.ts` runs one Effect fiber per
    instance that subscribes, loads state after `ready`, applies events and
    reconnects with the last sequences; `store.ts` holds the state and its
    reducers (idempotent, since events can arrive twice); `actions.ts` are the
    calls the UI makes; `saved.ts` is the instance list kept in localStorage.
  - `src/components/`, `src/pages/`: the UI. Routes are
    `/<instance>/<server>/<channel>`, where `<instance>` is the host.

## Rules

- Every change to a community server is a `ServerDb::write` that pushes at least
  one event payload and keeps the usage totals in step: members and channels in
  the `usage` row, message totals through `servers::add_usage`.
- Writes can run more than once (after a clash), so the closure given to
  `ServerDb::write` or `db::write` does nothing outside its transaction. Reads
  inside a write aren't checked for clashes, only rows written: a check that
  must hold under racing writes (a cap, "first account") needs every such write
  to update the same row, or one lock. A change that sweeps rows other writes
  may be adding to (a channel's messages) uses `ServerDb::write_alone`.
- Never write outside a transaction or with `BEGIN`/`BEGIN IMMEDIATE`: those
  lock out concurrent commits. Schema changes go in migrations, which run
  before anything else touches the file.
- Timestamps are unix milliseconds in the database, `google.protobuf.Timestamp` on
  the wire. Ids are ULIDs (`id::new_id`).
- Limits are unlimited unless configured. Never hardcode a usage cap.
- Everything about an instance or a server must be configurable from the
  client. A new operator switch is a field in `InstanceSettings` (with its
  `FUWA_*` default) and a control in the app's settings, not only an
  environment variable. Only how the process starts (paths, port, keys) stays
  environment-only.
- The usage signal must stay anonymous: no content, names or ids of people or
  servers. The end-to-end test checks this.
- The web app talks only through the protocol; anything it needs from a server
  goes in `proto/` first, then `pnpm generate`.
- Every screen ships with its motion: things enter and leave with a spring,
  selections glide (`layoutId`), counts roll, renamed things swap, and presses
  and hovers answer. Use the springs and helpers in
  `web/src/components/motion.tsx` (`SwapText`, `Count`, `CountUp`) so the app
  moves alike everywhere, and keep it working with reduced motion.
- Before pushing: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, and `buf lint`; for `web/`, `pnpm build` then
  `cargo test --features web`.
