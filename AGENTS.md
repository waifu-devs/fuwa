# AGENTS.md: working on fuwa

## Layout

- `proto/fuwa/v1/`: the protocol (gRPC). `buf lint` must pass; the Rust code is
  generated from it at build time (`server/build.rs`, protoc is vendored).
- `proto/fuwa/cluster/v1/`: the internal protocol between the parts of a split
  instance (`cpb` in Rust). Never for clients: it isn't in the reflection
  descriptor set or `web/src/gen`.
- `e2ee/`: `fuwa-e2ee`, the end-to-end encryption for direct messages (MLS,
  RFC 9420, through OpenMLS), shared by every client and the server. Without
  features it only reads MLS message headers (`wire`), which is all the server
  uses; the `client` feature is a whole device (`Device`: keys, groups,
  encrypting, safety numbers). The design is in `docs/e2ee.md`.
- `e2ee-wasm/`: `fuwa-e2ee` for the web app, as WebAssembly. `pnpm wasm` (in
  `web/`) builds it into `web/src/e2ee/pkg` (not committed); the wasm-bindgen
  crate and CLI versions must match.
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
  - Agents (`api/agents.rs`, `AgentService`) are accounts of kind
    `ACCOUNT_KIND_AGENT` that a person makes (`accounts.owner_id`, `public`).
    Their token is a session in `sessions` that never expires
    (`expires_at = i64::MAX`, `last_active_at` 0 until first use); a new
    token deletes the old sessions. An agent can't own servers, join or apply
    by itself, use DMs or be an instance admin: someone with Manage Server
    adds it (`AddAgent`, on the shard, which finds the agent through
    `App::find_agent`, the cluster call `FindAgent`), and it skips rules
    (never `pending`). Deleting a person deletes their agents
    (`erase_account`). Who may make agents is the `agent_creation` setting.
  - `dms.rs`: direct messages (`dms.db`, on the directory): each device's
    public signature key and key packages (one-use, plus a last-resort one),
    each conversation's records in one order (MLS commits and messages, all
    ciphertext), its latest GroupInfo and the welcomes waiting for new
    devices. A record is taken only for the conversation's current epoch (the
    `UPDATE ... WHERE epoch = ?` decides races), and a commit moves the epoch
    on. Devices belong to sessions: signing out (or the session ending) drops
    the device, and the hourly sweep clears any left over. `api/dms.rs`
    (`DirectMessageService`) checks what the server can see (who sends, the
    group id, epoch and content type in the MLS header, that key packages
    name the account and device they claim) and never decrypts anything.
  - `twofactor.rs`: TOTP codes (RFC 6238) and backup codes for two-step sign-in.
  - `linked.rs`: signing in with waifu.dev (linked accounts): the instance is an
    OpenAuth client whose client ID is its public URL. `AuthService`'s
    Start/Get/FinishLinkedSignIn (`api/auth.rs`) keep each sign-in in node.db's
    `linked_sign_ins` until the code comes back; linked accounts are keyed by
    `(linked_issuer, linked_subject)` and have no password, so two-step sign-in
    and password changes don't apply to them. The web app's side is
    `web/src/lib/linked.ts`, the "Continue with waifu.dev" button in
    `Connect.tsx` and the callback page (`pages/LinkedCallback.tsx`), which also
    hands sign-ins started by a fuwa app on another address back to it.
  - `servers.rs`: community servers, one Turso file each under `servers/`: the
    ones this process keeps (all of them, or a shard's share). Every change goes through
    `ServerDb::write`, which appends events to the server's log in the same
    transaction and publishes them to the `Hub` after commit. What owners and
    admins do also goes in the server's `audit` table (`servers::Audit`),
    kept apart from the event log so reasons for kicks and bans reach only
    managers. Invites live in the server file too (`invites`, made and
    revoked in `api/invites.rs`); the directory's index maps each code to its
    server, so `InviteService.GetInvite` finds one by code alone
    (`App::index_invite`, `App::describe_invite`). Who may come in
    (Browse or an invite, bans, account age, waifu.dev only) is checked in
    one place, `api/servers.rs`'s `at_the_door`, for joining and applying
    alike. Rules, application questions and applications live in the server
    file too, served by `api/join.rs` (`JoinService`): a member who joined
    while the server has rules is `pending` and loses the talking
    permissions (`permissions::TALK`) until they agree; an approved
    applicant agreed when they applied. Applications are seen only by
    members who can kick (`events::shown_to`).
    The welcome screen (`server.welcome`, JSON) is served by `JoinService`
    too: members get only the channels they can see. Custom emoji live in
    the server file (`emojis`, `api/emoji.rs`); their pictures are uploads
    (`MEDIA_PURPOSE_EMOJI`) counted in the server's attachments, and every
    change sends the whole list (`EmojisUpdated`). Messages write them
    `<:name:id>` (`<a:name:id>` when they move).
  - `webhooks.rs`: posting through a webhook over plain HTTP
    (`POST /webhooks/<server id>/<webhook id>/<token>`, a Discord-shaped JSON
    body), with each webhook's 30-a-minute limit (counted only for posts
    with the right token). Served where servers are kept; gateways pass these
    on to the shard holding the server (`Gateway::pass_to_shard`).
    `api/webhooks.rs` keeps the webhooks (`webhooks`, in the server file,
    tokens in the clear like invite codes; changes are audit-only) and posts
    the message (`execute_webhook`): its `author_id` is the webhook's id and
    `Message.webhook` carries the name and picture it posted under. Webhook
    messages never ping @everyone, @here or roles, and nobody can edit them.
  - `automod.rs`: what an AutoMod rule catches (words with `*` wildcards,
    pings, links to sites not allowed); `api/automod.rs` keeps the rules
    (`automod_rules`, one protobuf blob each) and `review` runs them inside
    the write that sends or edits a message: it blocks (an error starting
    "AutoMod: "), posts an alert message (`MESSAGE_KIND_AUTO_MOD_ALERT`) and
    times the author out. People with Manage Server are never caught.
  - `permissions.rs`: roles and permissions. `Rules::access` works out what a
    member may do (an `Access`): server-wide from their roles, and per
    channel by applying the category's overwrites and then the channel's
    (@everyone, then the member's roles together, then the member). No View
    Channels in a channel means no permissions there at all. `Access` also
    knows rank (the member's highest role; the owner above everything) for
    `outranks`, `above` and `may_change`.
  - `db.rs`: Turso helpers: opening, `user_version` migrations, transactions.
    Every database runs in Turso's concurrent-writer mode (MVCC): writes are
    `BEGIN CONCURRENT` transactions that run side by side and are retried
    when two touch the same row. An open file is a `Db`, whose gate
    (`shared` for writes, `alone` for folding the log in) lets the replica
    hold writes for a moment.
  - `replica/`: a split instance's continuous backup to a bucket or a folder
    (`FUWA_S3_*`, `FUWA_REPLICA_PATH`) and restoring from it (`FUWA_RESTORE`,
    `fuwa restore`): the directory's node.db, dms.db and pictures, each shard's
    servers under its name. It ships each file's MVCC log as it grows and
    folds the log in itself. `docs/storage.md` is the one design doc for
    storage, with the phases after this one. `s3.rs` is a small S3 client
    (Signature V4), `store.rs` a bucket or a folder behind one interface. A
    single process (`FUWA_ROLE=all`) refuses the settings: it keeps plain
    local files.
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
    directory's in-memory index of every server, its members, its invite
    codes and its shard.
    `calls.rs` holds every call that crosses parts, as `App` methods that run
    locally in one process and over `proto/fuwa/cluster` when split
    (`app.authenticate`, `server_changed`, `membership_changed`,
    `server_gone`, `check_picture`, `create_server`, `update_user`,
    `forget_account`, `describe_servers`…). `directory.rs` and `shard.rs`
    answer the internal protocol; `gateway.rs` routes client calls (by
    service, and by the `server_id` in field 1 for server-scoped ones) and
    merges live streams. `require_key` and `WithKey` are the only places the
    cluster key is checked and sent. A directory whose folder still has a
    single process's `servers/` hands them to the first shard that asks
    (`TakeServers`, then `HandedOver`; `shard::take_servers` runs before a
    shard opens its servers), keeping its copies in `handed-over/`.
    Restarts go unnoticed: gateways retry a call whose part is unreachable
    (or answered with the `fuwa-not-ready` header) for up to
    `ClusterConfig::ride_out` (`RIDE_OUT`, 30s), and re-follow a shard's
    servers from each one's last sequence when its live stream ends early;
    shards ask the directory read-only questions through `Link::ask`, which
    waits the same way; a directory that just started turns client calls
    away with `fuwa-not-ready` until every known shard has registered again
    (`Shards::caught_up`). A gateway's `/healthz` fails until it has reached
    the directory.
  - `web.rs`: serves the embedded web app (feature `web`, from `web/dist`), with
    `index.html` for any path the API doesn't answer so deep links work.
  - `tests/api.rs`: end-to-end tests against a running instance; `tests/web.rs`
    covers the embedded app; `tests/cluster.rs` runs a directory, two shards
    and a gateway and drives them through the gateway; `tests/replica.rs`
    loses a split instance's volumes and brings it back from its replica.
- `desktop/`: the desktop app (`fuwa-desktop`), native Rust with GPUI Kit,
  for Windows, macOS and Linux. Its own Cargo workspace (the root one
  excludes it), so GPUI stays out of the server's lock file and reproducible
  build. It talks to any instance over the same gRPC API as the web app, as
  gRPC-Web (`tonic-web`), and generates its client from `proto/fuwa/v1` in
  `build.rs`.
  - `src/core/`: everything that isn't drawing, ported from `web/src/fuwa` and
    `web/src/e2ee`: `api.rs` (clients, the `rpc!` macro, addresses),
    `store.rs` (state and reducers), `sync.rs` (one task per instance:
    subscribe, snapshot, apply, reconnect), `dms.rs` and `vault.rs` (one MLS
    device per install and account, through `fuwa-e2ee`'s `client` feature,
    kept in 0600 files under the app's data folder; signing out wipes it),
    `linked.rs` (waifu.dev sign-in through the browser and a loopback page),
    `config.rs` (saved instances and the app's settings). It runs on its own
    Tokio runtime and knows nothing of GPUI; the window watches its version.
  - `src/ui/`: the window. `app.rs` holds what's open and the overlays;
    `rail.rs`, `sidebar.rs`, `chat.rs`, `connect.rs`, `settings.rs`,
    `overlay.rs` draw the parts; `motion.rs` is how things move (springs,
    rises, glides, all settling at once with reduced motion); `theme.rs` is
    the web app's palettes and the bundled font (M PLUS Rounded 1c, whose
    files name the family "Rounded Mplus 1c").
  - `tests/core.rs`: two app cores against an in-process instance: servers,
    live messages, unread counts, encrypted DMs both ways, and that no
    plaintext reaches the instance's files.
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
  - `src/e2ee/`: encrypted direct messages in the browser. `engine.ts` is one
    device per signed-in account and instance (`DmEngine`): it registers the
    device, keeps key packages topped up, reads each conversation's records
    in order, adds and removes devices before sending, and joins by itself
    when nobody added it. `vault.ts` keeps the device's state and what it has
    read in IndexedDB (plaintext never goes back to the instance, and can't be
    decrypted twice, so this is the only copy); one tab works at a time (Web
    Locks) and tells the others. Signing out wipes the vault.
    `src/fuwa/dms.ts` are the actions; the screens are in `components/dm/`
    (`DmList`, `DmView`, `EncryptionDialog` with the safety number), routed at
    `/<instance>/dm/<conversation>`.
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
    and Access (rules and questions, welcome screen, roles, channels and their permissions,
    emoji, invites, AutoMod, applications, members, bans, audit log, ownership), shown by `dialogs/ServerSettingsDialog.tsx`,
    which also says which pages your permissions open (`useServerSettingsTabs`).
  - Invites: `dialogs/InviteDialog.tsx` makes and copies a link (from the
    server menu or a channel), `pages/InvitePage.tsx` is where a link lands
    (`/invite/<code>` on the instance, which goes to `/<instance>/invite/<code>`),
    and `src/lib/invites.ts` holds the choices, link format and paste parsing.
  - Getting in: `components/join/JoinButton.tsx` is the one button for it on
    Browse cards and invite pages (Join, Apply to join, waiting, waifu.dev
    only), `ApplyDialog.tsx` the application, `Rules.tsx` the rules sheet a
    new member agrees to (the composer shows it until they do), and
    `Applied.tsx` the applications waiting in the rail. `Welcome.tsx` greets
    new members once with the welcome screen (remembered in this browser). Those are kept in
    this browser (`src/lib/applied.ts`) and `AppliedWatcher` asks the
    instance how they went. Reviewers use `settings/server/Applications.tsx`;
    owners write rules and questions in `settings/server/JoinFormEditor.tsx`.
    `components/ModerateDialog.tsx` is the one dialog for nicknames,
    time-outs, kicks and bans, from the Members page and from profile cards;
    `components/MemberRoles.tsx` hands roles out wherever a member is shown.
  - `src/lib/permissions.ts`: the server's permission math (`accessOf`,
    `hasIn`, `above`, `mayChange`) and the labels for each permission. UI
    asks `useAccess` from `fuwa/hooks.ts` what you may do and hides what you
    can't, but the server decides.
  - `src/components/chat/mentions.tsx`: mentions in messages as chips (a remark
    plugin for `Markdown`), and `ServerLook`, the roles and members a server's
    messages need to color names. `MentionPicker.tsx` is the @ list in the
    composer; roles go in as `@Name` and are sent as `<@&id>`.
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
  - `src/lib/hosted.ts`: which addresses Waifu Devs runs (fuwa.chat).
    `components/HostedBadge.tsx` shows "Hosted by Waifu Devs" for those alone,
    reached over https, on the welcome screen, the instance home and sidebar,
    and invite pages. Only the address decides it, never anything an instance
    says about itself.
- `.railway/railway.ts`: fuwa.chat, the instance Waifu Devs hosts, in its own
  Railway project ("fuwa"): the published image, a volume at `/data`, the
  domain. Its `SPLIT` setting turns it into a directory (on that volume),
  shards (a volume each) and gateway replicas; shards can be added, never
  removed. Every merge to master redeploys each service it declares onto the
  new image (the `Deploy fuwa.chat` job in `publish.yml`); with a volume
  attached, Railway stops the old deployment before starting the new one, so
  while it's one process each deploy briefly drops connections. Split, only
  the directory and shards have volumes, and the gateways (which overlap old
  and new) ride out their restarts. A pull request that touches it gets a plan comment and merging
  applies it (`.github/workflows/railway-config.yml`); the root `package.json`
  exists only for this. The encryption key is a shared variable set by hand and
  must never change; so is the cluster key, which can. Don't edit the project in Railway's dashboard between a
  plan and its apply, or the apply refuses.

## Rules

- Every change to a community server is a `ServerDb::write` that pushes at least
  one event payload and keeps the usage totals in step: members and channels in
  the `usage` row, message totals through `servers::add_usage`. Invites are the
  exception: their codes are secrets, so making or revoking one writes only an
  audit entry, never an event. AutoMod rules are the same: members mustn't
  see the words a rule looks for.
- Writes can run more than once (after a clash), so the closure given to
  `ServerDb::write` or `db::write` does nothing outside its transaction. Reads
  inside a write aren't checked for clashes, only rows written: a check that
  must hold under racing writes (a cap, "first account") needs every such write
  to update the same row, or one lock. A change that sweeps rows other writes
  may be adding to (a channel's messages) uses `ServerDb::write_alone`.
- Never write outside a transaction or with `BEGIN`/`BEGIN IMMEDIATE`: those
  lock out concurrent commits. Schema changes go in migrations, which run
  before anything else touches the file.
- Every write to an open file holds its gate: `db::write`, `ServerDb::write`
  and `write_alone` do. With a replica on, only the replica folds a tracked
  file's log in (Turso's own checkpoint is off for it), so anything else that
  checkpoints takes `Db::alone`, and a new database file gets
  `replica.track` (servers go through `Servers::replicate`).
- Permissions, not ranks, decide what someone may do: a handler takes a `Seat`
  from `Api::with(account, server_id, Permission)` (or `membership` plus
  `access.require_in(channel, ...)` for channel ones). Rank only decides who
  may act on whom: nobody moderates, or changes a role, at or above their own
  highest role (`outranks`, `above`), and nobody grants or takes away a
  permission they don't have themselves (`may_change`), unless they own the
  server or are an Administrator. The client mirrors this in
  `web/src/lib/permissions.ts`. Every moderation or settings change writes an
  audit entry in the same transaction.
- A channel the member can't see doesn't exist for them: calls answer
  NotFound, lists leave it out, and their event stream drops its events. An
  event that changes what someone can see (roles, channels, their own member)
  sends them ChannelCreated and ChannelDeleted with sequence 0 for the
  channels that appear and go. Channel overwrites hold only the permissions
  `permissions::CHANNEL` lists; the rest are server-wide.
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
  build, and one build covers every part of a split instance.
- Every part of a split instance (`FUWA_ROLE`) is the same binary; the part is
  configuration, never a build feature. Handlers that take a `server_id` run
  on the shard holding it, so they read only that server's file and reach
  accounts, other servers, the index and pictures through the `App` methods
  in `cluster/calls.rs`, never `app.node()` or `app.index` directly. Handlers
  without one run on the directory. A new server-scoped request keeps `server_id` as field 1, and a
  new RPC gets a line in `gateway::route`; `every_call_is_routed` checks both.
  A part must be able to restart without clients noticing: only retry what
  can't have been done twice (a call that never reached its part, or one
  turned away with `fuwa-not-ready`), and use `Link::ask` only for questions.
  Every deploy restarts parts, so a part being out of reach is logged as info
  while it's waited for, and as a warning only once something gives up: a
  call tried again reads another part's failure with `Error::retried`, not
  `From<Status>` (which warns).
- Direct messages are end-to-end encrypted, always: no off switch, no
  server-side copy of keys or plaintext, nothing about their content in logs,
  events, exports or the usage signal. The server checks only what it can
  see in the clear (`docs/e2ee.md` lists it). A new kind of direct-message
  content goes in `DirectMessageContent` (inside the encryption), never as a
  new server field.
- Releases: set the version in `server/Cargo.toml`, merge, then push the tag
  `v<version>`. `Publish image` tags the image (`0.2.0`, `0.2`; `latest`
  follows master) for x86 and ARM, and `Release` makes the GitHub release with
  the binaries, `SHA256SUMS` and their attestations. Keep
  `docs/self-hosting.md` in step with anything self-hosters set up (variables,
  ports, image tags, the proxy).
- Before pushing: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, and `buf lint`; for `web/`, `pnpm wasm` and `pnpm build`, then
  `cargo test --features web`; for `desktop/`, the same three cargo commands
  run inside `desktop/` (`cargo fmt`, not `--all`).
- The desktop app and the web app are two faces of one client: a feature,
  setting or fix in one belongs in the other too, and the desktop's core
  mirrors `web/src/fuwa` and `web/src/e2ee` file for file where it can. Its
  settings are this computer's and apply to every instance, like
  `web/src/lib/prefs.ts`. Pictures in messages show as links, as on the web.
