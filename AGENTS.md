# AGENTS.md: working on fuwa

## Layout

- `proto/fuwa/v1/`: the protocol (gRPC). `buf lint` must pass; the Rust code is
  generated from it at build time (`server/build.rs`, protoc is vendored).
- `proto/fuwa/cluster/v1/`: the internal protocol between the parts of a split
  instance (`cpb` in Rust). Never for clients: it isn't in the reflection
  descriptor set or `web/src/gen`.
- `server/`: the Rust server (`fuwa` binary, `fuwa_server` library).
  - `app.rs`: shared state, the HTTP router (gRPC, gRPC-Web, CORS, health), serving.
  - `api/`: one file per gRPC service, all implemented on `Api`.
  - `node.rs`: the instance database (`node.db`): accounts and profiles,
    sessions (devices), two-step sign-in (TOTP secrets, backup codes, sign-in
    tickets), notification settings, meta (the install id, the announcement).
    Admins can turn an account off (`disabled_at`): it loses its sessions and
    can't sign in until it's turned back on. What belongs to a person but not to
    one server lives here; a server file keeps only a copy of what its members
    see (name, avatar, status) in its `users` table.
  - `twofactor.rs`: TOTP codes (RFC 6238) and backup codes for two-step sign-in.
  - `servers.rs`: community servers, one Turso file each under `servers/`: the
    ones this process keeps (all of them, or a shard's share). Every change goes through
    `ServerDb::write`, which appends events to the server's log in the same
    transaction and publishes them to the `Hub` after commit. What owners and
    admins do also goes in the server's `audit` table (`servers::Audit`),
    kept apart from the event log so reasons for kicks and bans reach only
    managers.
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
  - `media.rs`: uploaded pictures (avatars, banners, server icons), one file
    each under `media/`, with a row in node.db's `media` table. The HTTP side
    lives here: `PUT /media/upload/<token>` takes the file for an upload
    reserved with `MediaService.CreateUpload` (`api/media.rs`) and checks its
    bytes really are the picture type it claims; `GET /media/<id>` serves it.
    Pictures nothing uses are swept hourly and at startup.
  - `migrations/node`, `migrations/server`: SQL applied in order, tracked in
    `PRAGMA user_version`. Never edit a migration that has shipped; add a new file
    and list it in `MIGRATIONS`.
  - `cluster/`: running as separate parts (`FUWA_ROLE`). `index.rs` is the
    directory's in-memory index of every server, its members and its shard.
    `calls.rs` holds every call that crosses parts, as `App` methods that run
    locally in one process and over `proto/fuwa/cluster` when split
    (`app.authenticate`, `server_changed`, `membership_changed`,
    `server_gone`, `check_picture`, `create_server`, `update_user`,
    `forget_account`, `describe_servers`…). `directory.rs` and `shard.rs`
    answer the internal protocol; `gateway.rs` routes client calls (by
    service, and by the `server_id` in field 1 for server-scoped ones) and
    merges live streams. `require_key` and `WithKey` are the only places the
    cluster key is checked and sent.
  - `web.rs`: serves the embedded web app (feature `web`, from `web/dist`), with
    `index.html` for any path the API doesn't answer so deep links work.
  - `tests/api.rs`: end-to-end tests against a running instance; `tests/web.rs`
    covers the embedded app; `tests/cluster.rs` runs a directory, two shards
    and a gateway and drives them through the gateway.
- `web/`: the web app (pnpm, Vite, React 19, TanStack Router, Tailwind 4,
  shadcn/ui and Animate UI copied from the waifu.dev site, Effect).
  - `src/gen/`: protobuf code from `pnpm generate`. Generated, committed, never
    edited by hand.
  - `src/fuwa/`: talking to instances. `sync.ts` runs one Effect fiber per
    instance that subscribes, loads state after `ready`, applies events and
    reconnects with the last sequences, and reads the instance's public
    details (name, sign-ups, announcement) again every minute, since those
    change without an event; `store.ts` holds the state and its
    reducers (idempotent, since events can arrive twice); `actions.ts` are the
    calls the UI makes; `saved.ts` is the instance list kept in localStorage.
  - `src/components/`, `src/pages/`: the UI. Routes are
    `/<instance>/<server>/<channel>`, where `<instance>` is the host (or, in
    streamer mode, a local alias like `/~waifu-devs`, see `lib/streamer.ts`).
  - `src/lib/prefs.ts`: app settings, which belong to this device and apply to
    every instance (theme, density, keybinds, streamer mode...). Settings of
    an instance or a server live on that instance instead.
  - `src/lib/notifications.ts`: how a message reaches you: your settings for
    its channel, then its server (both stored on the instance, so they follow
    you across devices), then this device's Notifications settings. Muted means
    no sound, no notification and no unread badge.
  - `src/components/settings/account/`: the "Your account" pages (profile,
    server profiles, devices, two-step sign-in, server notifications, data).
  - `src/components/settings/server/`: server settings pages beyond Overview
    (channels, members, bans, audit log, ownership), shown by
    `dialogs/ServerSettingsDialog.tsx`. `components/ModerateDialog.tsx` is the
    one dialog for nicknames, time-outs, kicks and bans, from the Members page
    and from profile cards.
  - `src/components/settings/instance/`: the instance admin pages beyond
    settings (accounts, servers, announcement), shown by
    `settings/InstanceSettingsDialog.tsx`. The announcement itself is drawn by
    `components/AnnouncementBanner.tsx`, above the app for the instance you're on.
  - `src/components/PictureField.tsx`: the one field for uploading an avatar,
    banner or server icon (drop or pick, crop in `PictureCropper.tsx`, upload
    with progress, or a link). `src/lib/pictures.ts` holds the crop math.
  - `src/lib/keybinds.ts`: every keyboard action and its default; the key
    handler (`components/Shortcuts.tsx`), the shortcut sheet and the Keybinds
    page all read this one list.

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
- Moderation follows rank: owners outrank admins, admins outrank members, and
  nobody acts on someone at or above their own rank (`outranks` on both the
  server and the client). Every moderation or settings change by a manager
  writes an audit entry in the same transaction.
- Messages have a `kind`. Anything that isn't a plain message (join messages
  today) has empty content, can't be edited, is left out of data exports, and
  never plays a sound or shows a notification.
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
- Anything that can show an instance's address or your own username goes
  through `Private` or `usePrivateField` (`components/Private.tsx`), so
  streamer mode hides it. Secrets (a two-step key, backup codes) blur whenever
  streamer mode is on.
- Anything that weakens an account (turning off two-step sign-in, new backup
  codes, deleting it) asks for the password again, and a code when two-step
  sign-in is on.
- Instance admins act on other accounts, never their own (no turning yourself
  off, resetting your own password or removing your own admin), and an
  instance always keeps at least one admin. Admins are demoted before they're
  turned off.
- The web app talks only through the protocol; anything it needs from a server
  goes in `proto/` first, then `pnpm generate`. The one exception is sending an
  upload's bytes, a plain `PUT` to the link `CreateUpload` hands out.
- A picture link that points at one of the instance's own uploads is checked
  before it's saved (`Api::check_picture`: yours, the right kind, finished),
  marked used after (`keep_picture`), and the picture it replaced is deleted
  (`drop_picture`). Any new field that takes a picture does the same.
- Every screen ships with its motion: things enter and leave with a spring,
  selections glide (`layoutId`), counts roll, renamed things swap, and presses
  and hovers answer. Use the springs and helpers in
  `web/src/components/motion.tsx` (`SwapText`, `Count`, `CountUp`) so the app
  moves alike everywhere, and keep it working with reduced motion.
- Builds are reproducible: the same commit gives the same binary, byte for
  byte, and the `Reproducible build` workflow fails a change that breaks this
  (it's slow, so for now it only runs when started by hand from Actions; run it
  on any change to the Dockerfile, `build.rs` or dependencies).
  Nothing that differs between builds goes in the binary: no build time, no
  random value, no absolute path (the commit is fine, it's in `FUWA_COMMIT`
  from `build.rs`). Keep rust-embed's `deterministic-timestamps` and the
  Dockerfile's `SOURCE_DATE_EPOCH`, and pin any new base image by digest.
  This is what will let clients check, later, that a server runs an official
  build, and one build covers every role.
- Every role is the same binary; the role is configuration, never a build
  feature. Handlers that take a `server_id` run on the shard holding it, so
  they read only that server's file and reach accounts, other servers, the
  index and pictures through the `App` methods in `cluster/calls.rs`, never
  `app.node()` or `app.index` directly. Handlers without one run on the
  directory. A new server-scoped request keeps `server_id` as field 1, and a
  new RPC gets a line in `gateway::route`; `every_call_is_routed` checks both.
- Before pushing: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, and `buf lint`; for `web/`, `pnpm build` then
  `cargo test --features web`.
