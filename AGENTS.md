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
  encrypting, safety numbers). The design is in `docs/e2ee.md`; secure
  channels (end-to-end encrypted channels in servers) use the same devices
  and groups, see `docs/secure-channels.md`.
- `e2ee-wasm/`: `fuwa-e2ee` for the web app, as WebAssembly. `pnpm wasm` (in
  `web/`) builds it into `web/src/e2ee/pkg` (not committed); the wasm-bindgen
  crate and CLI versions must match.
- `sdk/`: `@waifu-devs/fuwa`, the TypeScript SDK (its own pnpm package, like
  `web/`; docs/sdk.md). `src/gen` is generated from `proto/fuwa/v1` with the
  web app's buf setup plus `.js` import paths (`pnpm generate`, checked in
  CI). `client.ts` makes the typed clients (`createFuwa`: auth, typed errors
  from `errors.ts`, retries from `retry.ts`), `events.ts` the reconnecting
  `EventFollower`, `agent.ts` the `Agent` (commands, mentions, typed event
  handlers), `voice.ts` voice channels over ListenVoice/SpeakVoice (no WebRTC), `ogg.ts`
  Ogg Opus files, `pages.ts`, `upload.ts` and `text.ts` the helpers. It never logs
  tokens or addresses and reports nothing. `test/agent.test.ts` drives a real
  instance (`FUWA_BIN`); new agent-facing calls get a helper and a test there.
- `voice/`: `fuwa-voice`, the client crate programs use to hear and talk in
  voice channels (`ListenVoice` and `SpeakVoice`, no WebRTC), with the
  `parrot` example. The server's tests use it against a real instance.
- `server/`: the Rust server (`fuwa` binary, `fuwa_server` library).
  - `app.rs`: shared state, the HTTP router (gRPC, gRPC-Web, CORS, health), serving.
  - `api/`: one file per gRPC service, all implemented on `Api`.
  - `node.rs`: the instance database (`node.db`): accounts and profiles,
    sessions (devices), two-step sign-in (TOTP secrets, backup codes, sign-in
    tickets), notification settings, server arrangements (each person's rail
    order and folders, `AccountService.Get/SetServerArrangement`; the web side
    is `lib/rail.ts`, `components/RailFolder.tsx` and `hooks/use-rail-arrange.ts`),
    meta (the install id, the announcement).
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
    hands sign-ins started by a fuwa app on another address back to it. Only
    apps the instance trusts get sign-ins back (`linked::return_origin`): its
    own address, loopback (desktop apps), and origins listed by name in
    `allowed_origins`.
  - `sso/`: single sign-on through an identity provider, for the instance's
    own sign-in and for one community server's. `mod.rs` is the shared
    `Provider` (stored as JSON: node.db settings' `sso_provider`, or the server
    file's `server.sso`), `Endpoints` (every URL from the public URL, never the
    request), and start/identify/finish over a `sso_sign_ins` table (node.db
    for the instance, the server file for a server). The instance's sign-ins
    get a row only once the provider answers: until then the state is a
    signed ticket (`ticket.rs`, under the picture-link key) that carries
    everything, since anyone may start one. `oidc.rs` is the code
    flow with PKCE and ID token checks against the JWKS; `saml.rs` the
    HTTP-Redirect AuthnRequest and the signed-response checks, over the
    hand-written exclusive C14N in `xml.rs` (fixtures signed by an
    independent library are in `sso/testdata/`). `http.rs` takes providers'
    answers at `/sso/instance/...` and `/sso/servers/<id>/...` and sends the
    browser to `/auth/sso/done#...`. Server-scope providers are fetched only
    from public addresses (`outside::PublicOnly`, shared with picture
    fetches). The instance's flow is `AuthService`'s
    Start/Get/FinishSsoSignIn (SSO accounts, kind `SSO`, keyed by the
    provider and subject); a server's is `api/sso.rs` (`SsoService`),
    whose sign-ins live in the server file's `sso_identities`. Only the
    server's owner may set it up, at https; `Provider::trust_key` (key,
    client ID, sign-in URL, certificate fingerprints) decides when it's a
    new provider, while `key` alone names instance SSO accounts. A required
    provider gates `let_in` and, through `permissions::Access::lock_out`,
    hides every channel from members whose sign-in is missing or older than
    `sso_recheck_days` (`app::spawn_sso_rechecks` tells their streams when one
    runs out). The web side is `web/src/lib/sso.ts`, `pages/SsoDone.tsx`,
    `components/settings/IdentityProviderForm.tsx` (both levels),
    `settings/server/SingleSignOn.tsx` and `components/join/SsoGate.tsx`.
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
    Threads (`api/threads.rs`, docs/threads.md): a reply is a message with
    `thread_id` (its parent) and `in_channel`; `threads` sums each one up
    and every send or delete of a reply calls `threads::refresh` in the
    same write (`ThreadUpdated`). Paths that delete messages call
    `threads::after_delete`; shared channels strip threads for guests.
    Shared channels (`api/shared.rs`, docs/shared-channels.md): a channel's
    home keeps it and every message (`channel_guests`, `share_codes`,
    `channel_blocks`); a guest server shows it as a channel of its own
    (`channel_links`) and keeps none of it. The guest's shard checks its own
    roles and AutoMod and passes reads and writes to the home as
    `cluster.v1.SharedCall`s (`App::shared`); the home's
    `spawn_shared_fanout` passes message events back, published at the guest
    as sequence 0. Message calls on a channel check `shared::link_of` first;
    new channel kinds or message paths must too.
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
  - `mcp/`: the instance as an MCP server for agents (`docs/mcp.md`), at
    `/mcp` and `/.well-known/mcp.json`: stateless Streamable HTTP, one
    JSON-RPC message per POST, plain JSON back, no sessions. Agent tokens
    only, a token bucket per agent account (`Limits`). Every tool
    (`tools.rs`) is a public gRPC call made in process through the same
    router clients reach (`call!` over `Mcp::inner`: the gateway's when
    split), so routing, permission checks and limits are the call's own;
    it never touches a database. `view.rs` turns messages into compact
    JSON, `catalog.rs` holds the resources and prompts. Before any tool on
    a server it asks `AgentService.GetMcpAccess` (the server file's
    `mcp_access`) whether that server's managers let the agent in. A new
    API call agents should have gets a tool here, wrapping that call.
  - `automod/`: what an AutoMod rule catches (words with `*` wildcards,
    pings, links to sites not allowed); `api/automod.rs` keeps the rules
    (`automod_rules`, one protobuf blob each) and `review` runs them inside
    the write that sends or edits a message: it blocks (an error starting
    "AutoMod: "), posts an alert message (`MESSAGE_KIND_AUTO_MOD_ALERT`) and
    times the author out. People with Manage Server are never caught.
    `automod/providers.rs` is moderation services (`Provider`, registered in
    `KINDS`: TypeSafe Jev and Cloudflare Clef, which both speak System One,
    so one adapter): the instance's `automod_providers` setting holds their
    keys (never sent to clients; shards get them in `WatchResponse`), and a
    server's one PROVIDER rule ("Smart filter") picks one and a level per
    label. `api/automod.rs`'s `ask` calls it before the message's write
    (never inside it), with only the text (`providers::outgoing` strips
    mentions and emoji ids), cut off at 3 seconds; a failure lets the
    message through that rule and is counted in the anonymous report by
    kind and provider id. The web pages are `settings/instance/Moderation.tsx`
    and the Smart filter in `settings/server/AutoMod.tsx`.
  - `search.rs` and `api/search.rs`: searching a server's messages
    (`SearchService`). Turso's own full-text search needs a new dependency
    and doesn't run in MVCC, so the index is plain tables in the server's file
    (`search_words`, `search_docs`, `search_postings`, `search_state`).
    `search.rs` cuts text into words (folded case, accents and full-width
    forms; CJK as pairs of characters; `@name` mentions; attachment names and
    embeds) and is pure; `VERSION` going up rebuilds every index. One
    indexer per shard (`spawn_search_indexer`) follows each server's events
    through `Hub::search_tap` and builds servers that had messages before
    search, newest first, in small `write_quiet` batches between other
    writes. A search ANDs the words (the last one as a prefix), filters by
    author, channel, mention, what a message has and dates, only in channels
    the searcher can see, and re-reads each hit from `messages`. Searches are
    never logged, stored or shown to AutoMod; a token bucket per account
    limits them. Secure channels and other instances' shared messages aren't
    indexed.
  - `permissions.rs`: roles and permissions. `Rules::access` works out what a
    member may do (an `Access`): server-wide from their roles, and per
    channel by applying the category's overwrites and then the channel's
    (@everyone, then the member's roles together, then the member). No View
    Channels in a channel means no permissions there at all. A member who's
    timed out keeps only View Channels until it ends (`Access::time_out`), so
    they read and can only leave or agree to the rules. `Access` also
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
  - Calls (`docs/calls.md` is the design): `rtc.rs` is the SFU, str0m
    driven by one task (UDP and ICE-TCP on one port, ICE-lite), with
    `offer_allowed` and per-person budgets limiting what an app may send.
    `voice.rs` keeps who's in which call in memory (`Voice`, leases) and
    `MediaLink` reaches the media part (in process, or the media parts on
    `FUWA_MEDIA_URL` by rendezvous hashing). `api/calls.rs` is
    `CallService`: places, keeps, moderation (`voice_moderation` in the
    server file keeps server mute and deafen), TURN credentials, and
    `spawn_voice_guard`, which hangs people up as soon as an event takes
    their access away (through `Hub::tap`). `cluster/media.rs` is a media
    part (`FUWA_ROLE=media`). Programs join voice channels through a
    bridge in `rtc.rs` (`Sfu::bridge` and `Sfu::speak`): no WebRTC, Opus
    frames labelled by speaker, carried by `listen` in `api/calls.rs` across
    media restarts. Cameras are simulcast (rids l, m, h): `TrackOut` sends
    each viewer the size it asked for over the data channel ("layers"),
    switching on keyframes, and VIDEO gates them (`May` in `rtc.rs`). An
    app's second video track is its shared screen (`Source::Screen`), sent
    on a stream named `<account>-screen`. `recordings.rs` records voice
    channels on the server while anyone there has `server_record` on: a
    bridge per recording, a hand-written Ogg Opus file per speaker (frames
    untouched, silence between, every track lined up from the start),
    sealed with ChaCha20-Poly1305 under an HKDF key from
    FUWA_ENCRYPTION_KEY, rows in the server file's `recordings` table, and
    the finished files copied to the replica under `recordings/`.
    Calls in direct messages are end-to-end
    encrypted by the apps; the server never holds their keys and only
    forwards sealed frames. Never put a participant's address in a log or an event.
  - `config.rs`: `FUWA_*` environment variables: how the process starts, and
    the defaults for settings.
  - `settings.rs`: settings admins change from a client (`AdminService`), stored
    in node.db's `settings` table over the environment's defaults. Read them
    through `app.settings()`, never from `config`, so changes apply at once.
  - `telemetry.rs`: the anonymous usage signal (schema `fuwa.signal.v1`).
  - `reports.rs`: the hourly anonymous health report (`fuwa.report.v1`): one
    process-wide collector of errors (panics, `Error::Internal`/`Database`/`Io`
    answers, named by the gRPC method from `time_calls`), timings in fixed
    buckets and usage counts, plus what apps send through
    `NodeService.SendReport` (checked labels, one a minute per account, never
    kept who). Off with the telemetry switch. The web app's side is
    `web/src/lib/reports.ts`, the desktop's `desktop/src/core/reports.rs`.
  - `media.rs`: uploaded pictures (avatars, banners, server icons), one file
    each under `media/`, with a row in node.db's `media` table. The HTTP side
    lives here: `PUT /media/upload/<token>` takes the file for an upload
    reserved with `MediaService.CreateUpload` (`api/media.rs`) and checks its
    bytes really are the picture type it claims; `GET /media/<id>` serves it.
    Pictures nothing uses are swept hourly and at startup.
  - `outside.rs`: pictures from other sites. No client ever loads a picture
    from anywhere but a fuwa instance, since that site would learn the
    reader's IP address: any picture link that isn't an upload (embed images,
    webhook post avatars, the waifu.dev picture) is rewritten when stored, with
    `App::picture_link`, to `/media/outside/<hmac>?url=...`, which the instance
    fetches itself (public addresses only, pictures only, 8 MB, cached). The
    key is from FUWA_CLUSTER_KEY when split, else node.db's `meta`. New
    fields that hold a picture link go through `picture_link` too; the web
    app's `lib/shown.ts` hides any that don't.
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
    Regions (`docs/regions.md` is the design): every part may carry
    `FUWA_REGION`; the directory's is the home region and keeps each shard's
    (`shards.region`). A server's region is its shard's, stamped on its file
    when opened (`server.region`); `create_server` picks the emptiest shard
    of the region asked for (`Shards::emptiest_in`), never one elsewhere.
    `moves.rs` moves a server between regions for an admin
    (`AdminService.MoveServer`): the directory records it in `moves`, the new
    shard pulls the files from the old one (`SendServer`, which freezes it:
    writes answer `Error::Moving`, which gateways ride out), the directory
    switches the placement, then the old shard lets go (`ReleaseServer`,
    deleting its files, recordings and replica copies, and ending live
    streams with `Misrouted` so gateways follow) and the new one starts
    replicating. `sort_registration` keeps a restart mid-move from putting a
    server in two places.
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
    `linked.rs` (waifu.dev sign-in through the browser and a loopback page;
    the sign-in page must be https, or http on this computer), `sso.rs`
    (single sign-on the same way: an instance's provider on the sign-in
    screen, a server's to join it or to see its channels again; `locked` is
    the web's `ssoLocked`, and the UI names the provider's host, which sees
    the person's IP address, before opening it),
    `config.rs` (saved instances and the app's settings; no tokens),
    `themes.rs` (`docs/themes.md` in Rust: the five built-in themes,
    `deriveTokens`, made and imported themes, backdrops and theme files,
    everything read from a file or settings checked and clamped; a picture is
    only ever a `/media/` link on an instance), `backgrounds.rs` (background
    pictures kept on an instance, and their bytes for exporting a theme),
    `secrets.rs` (session tokens and the vault key in the system keychain,
    named per data folder, with a 0600 file only where there's no keychain;
    vault files are sealed with XChaCha20-Poly1305 under that key), `permissions.rs`
    (what you may do in a server, a port of `web/src/lib/permissions.ts`),
    `notifications.rs` (per channel and server levels and mutes, kept on the
    instance, and whether a message should notify), `account.rs` (profile,
    pictures, password, signed-in devices, rules, the welcome screen, creating
    channels), `moderation.rs` (time outs, kicks and bans, and who may do
    them to whom: the permission plus outranking them), `server_admin.rs`
    (a server's settings, invites, bans and audit log), `calls.rs` (who's in
    voice and which conversations have a call, and the direct-message call
    frame encryption, byte for byte the web app's; the call itself comes
    with the app's sound), `reports.rs` (the anonymous reports: panics, also
    written to `crashes.txt` in the config folder so a crash is sent next
    time, failed and timed `rpc!` calls, startup, catching up, slow frames
    and a few feature counts, sent every 10 minutes through
    `NodeService.SendReport` to one signed-in instance whose telemetry is
    on, kept for next time when that fails; off with the "Help fix bugs"
    setting, which counts nothing). It runs on its own
    Tokio runtime and knows nothing of GPUI; the window watches its version.
  - `src/ui/`: the window. `app.rs` holds what's open and the overlays;
    `rail.rs`, `sidebar.rs`, `chat.rs`, `connect.rs`, `settings.rs`,
    `overlay.rs` draw the parts (a server you're locked out of shows a
    padlock and "Continue with <provider>" where its channels were);
    `arrange.rs` drags channels and categories into order in the sidebar
    (Manage Channels), with GPUI's drag view, a drop line and a category
    ring, over the layout and `ReorderChannels` call in `core/arrange.rs`
    (the web's `lib/arrange.ts`); `members.rs` is the member list, a view of
    its own (cached, so the window's animations don't redraw it) that builds
    only the rows in sight, grouped under each hoisted role like the web's
    `MemberList.tsx`; `compose.rs` is the @ list and editing in
    place (and the keys they take first), `mentions.rs` finds mentions and
    makes them links, `menus.rs` the bell menus, `notify.rs` the system
    notifications (clicks come back through a channel), `settings_account.rs`
    the profile and security pages, `settings_look.rs` the Appearance
    (themes, light and dark picks, theme files) and Background pages,
    `settings_privacy.rs` the Privacy page ("Help fix bugs", what a report
    holds, and what's waiting to go out),
    `backdrop.rs` what's drawn behind the app (picture, blurred once off the
    main thread when asked, dimming, a texture made here as a PNG or SVG
    tile and repeated), `effects.rs` the moving effects (the web's shaders
    redrawn with shadows, paths and quads, 30 frames a second while the
    window is in front, one still frame otherwise, and none while a
    full-screen page like server settings covers them), `keys.rs` the
    keyboard shortcuts (one handler on the window, the quick switcher and
    the shortcut sheet) over `core/keybinds.rs` (the web's
    `lib/keybinds.ts` list and combo format, so a saved combo means the same
    in both), `settings_keys.rs` the Keyboard page where they're changed,
    `server_settings.rs` a server's settings
    (overview, welcome screen, invites, roles, channels, emoji, integrations, members,
    bans, AutoMod, audit log; the server's name opens it; a cached view, so
    it redraws only when the server changes, and its flourishes play once
    rather than loop; `save_bar` is the floating unsaved-changes bar pages
    share),
    `server_settings/roles.rs` the Roles page (order, color, permissions
    and members of each role, saved together from a floating bar, over the
    role calls in `core/server_admin.rs`; you edit only roles below your own
    and hand out only what you have, as the server checks),
    `server_settings/emoji.rs` the Emoji page (pictures dropped or picked,
    shrunk to 128 pixels and written as PNGs by `png.rs`, which compresses
    them itself so the app needs no image encoder), `server_settings/webhooks.rs`
    the webhooks on the Integrations page (a test post goes to the webhook's
    own instance only), under the agents in `server_settings/agents.rs`
    (added by username, removed by kicking), `server_settings/welcome.rs`
    the Welcome screen editor beside a preview drawn like the welcome
    dialog, `server_settings/automod.rs` the AutoMod rules (each tried with
    `TestAutoModRule` as it's edited, before it's saved; the Smart filter
    picks one of the instance's providers, a level and "how sure" per label,
    and pictures where the provider reads them),
    `server_settings/channels.rs` the Channels page (moved a place at a
    time with `core/arrange.rs`'s `step`, each one's name, topic, category
    and slow mode, and who can see and do what in it, saved as one
    `SetChannelPermissions`; the New button opens the app's new-channel
    dialog and stays in settings),
    `instance_settings.rs` an instance's settings for its admins (the gear
    by the instance's name; General, Sign-ups, Single sign-on
    (`instance_settings/sso.rs`: the identity provider, SAML metadata read
    by `core/sso.rs`, a test sign-in in the browser), Limits, Privacy,
    Calls and Moderation, where the providers servers' smart filters ask are set up
    and tried, and Other instances (`instance_settings/federation.rs`: the
    switch for sharing channels with other instances, this instance's key,
    checking another instance, who it has heard from and the blocked hosts),
    over `core/instance_admin.rs`, which names every setting's
    path and keeps unsaved edits across a save; the pages are built from
    `instance_settings/controls.rs`, the web's `settings/controls.tsx`:
    a setting with its default and reset, option cards, caps; a cached view
    like server settings, sharing its `save_bar`, `switch` and chips; its
    Manage group acts straight away instead: `instance_settings/accounts.rs`
    finds accounts and makes admins, resets passwords or turns accounts off,
    in a list that draws only the rows in sight,
    `instance_settings/servers.rs` lists every server and opens one to change
    its caps, move its region, end its shared channels, save its file or
    delete it (over `core/instance_servers.rs`), and
    `instance_settings/announcement.rs` puts up the banner, over
    `core/instance_manage.rs`),
    `announcement.rs` that banner across the top of the app (news, heads-up
    or urgent, which can't be closed; a closed one stays closed in prefs),
    `moderate.rs` the time out, kick and ban
    buttons and dialog; `emoji.rs` (the built-in list, server emoji tokens,
    the `:name:` list, and a Markdown plugin that draws emoji inline),
    `emoji_picker.rs` the picker by the composer, `embeds.rs` the cards apps
    post through webhooks; `http.rs` fetches pictures for `img` on the core's
    runtime (GPUI's own client loads nothing), only from fuwa instances
    (the ones you added, at either address they have), redirects included,
    like the web app's `lib/shown.ts`; instances fetch other sites' pictures
    themselves (`server/src/outside.rs`). Markdown goes through `text::markdown`,
    whose links open only for http(s) and mailto. `motion.rs` is how things move (springs,
    rises, glides, all settling at once with reduced motion; `ambient` loops
    run only while the window is in front); `theme.rs` is
    the theme on screen as a palette (with the surfaces `docs/themes.md`
    gives, see-through over a backdrop), `corner()` for corners that follow
    the theme's radius, and the bundled font (M PLUS Rounded 1c, whose
    files name the family "Rounded Mplus 1c"). `perf.rs` is the frame and
    memory meter: `FUWA_DESKTOP_PERF=1` logs startup, frames a second, frame
    build times and memory (debug level logs every frame). The whole window
    redraws on any change or animation frame, so keep big parts virtual or
    in cached views, and keep work that grows with a channel's length out of
    each change (`chat.rs` reuses built messages by signature).
  - `tests/core.rs`: two app cores against an in-process instance: servers,
    live messages, mentions that notify, edits, mutes kept on the instance,
    unread counts, time-outs and kicks reaching the person live, encrypted DMs both ways, and that no plaintext reaches the
    instance's files.
  - `packaging/`: the icon (`icon.svg`, and the PNGs and `.ico` made from it)
    for the installers. `[package.metadata.packager]` in `Cargo.toml` tells
    cargo-packager what to make, and `.github/workflows/desktop.yml` makes
    them: a `.deb` and AppImage for Linux, a `.dmg` for macOS, an NSIS
    `-setup.exe` for Windows. Release calls it for each tag.
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
  - `src/calls/`: calls, kept apart from `fuwa/store.ts` in their own
    store (`state.ts`). `engine.ts` is one call at a time (`Session`: join,
    keep, answer the media part's offers over the `fuwa` data channel,
    rejoin with the same session on a restart), `audio.ts` the microphone
    (gain, voice activity or push to talk, mute) and speakers (per-person
    volume, output device, who's speaking), `frames.ts` and
    `frames.worker.ts` the end-to-end encryption of direct-message calls,
    `keys.ts` push to talk. The screens are in `components/calls/`; the
    Voice & audio settings are `settings/app/Voice.tsx` and the instance's
    Calls page `settings/instance/Calls.tsx`.
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
  - `src/fuwa/search.ts`, `src/lib/search-query.ts`, `components/search/`:
    the search bar (Mod+F) and results panel. `search-query.ts` reads
    `from:`, `in:`, `has:`, `mentions:`, `before:`, `after:` and `during:`
    and keeps recent searches on the device only; members and channels are
    turned into ids before asking the instance. Results mark matches with
    private-use characters that `components/search/highlight.ts` turns into
    `<mark>` inside the one Markdown component. Jumping to a result goes
    through `requestJump`, which `MessageList` takes by loading older pages
    until the message is there.
  - `src/lib/prefs.ts`: app settings, which belong to this device and apply to
    every instance (theme, density, keybinds, streamer mode...). Settings of
    an instance or a server live on that instance instead.
  - Themes (`docs/themes.md` is the format every app shares): built-ins in
    `src/lib/themes.ts` (copied from waifu.dev), custom themes and theme files
    in `lib/theme-file.ts`, the backdrop (picture and effect) in
    `lib/backdrop.ts`, drawn by `components/Backdrop.tsx` behind `#root`.
    Shader effects are WGSL in `lib/effects/shaders.ts`, run by vgpu in
    `lib/effects/gpu.ts`, which is loaded only when an effect is on (it's
    kept out of the vendor chunk in `vite.config.ts`); CSS stands in without
    WebGPU. Custom shaders (people's own WGSL `fn shade`) are in
    `lib/effects/custom.ts` (the prelude, the text checks, the starters);
    `gpu.ts` compiles and times them before they draw, `lib/effects/status.ts`
    remembers which ran, were too slow or stopped the GPU, and anything but
    running shows the shader's fallback. Their editor is
    `settings/app/ShaderEditor.tsx`. Theme files never make the app load
    anything: pictures travel inside them and are uploaded on import. Settings
    pages: `settings/app/Themes.tsx` and `Backgrounds.tsx`.
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
  - Arranging channels: with Manage Channels, rows in the sidebar
    (`ChannelSidebar.tsx`) and in the Channels settings page drag into order,
    into and out of categories, through `hooks/use-arrange.ts`. It works on
    the DOM (rows marked `data-arrange`, `data-id`, `data-parent`) and moves
    a copy, the drop line and the category ring by `transform`, so the list
    renders only once, on the drop. `lib/arrange.ts` is the layout (loose
    channels, then each category and its channels), what a drop or an arrow
    key does to it, and the order `ChannelService.ReorderChannels` takes;
    `reorderChannels` shows the new order at once and puts it back if the
    server says no.
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
  removed. `fuwa-media` carries calls' sound, reached by apps through a TCP
  proxy (Railway has no public UDP), one replica. Every merge to master redeploys each service it declares onto the
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
  audit entry, never an event (shared channels' share codes too). AutoMod rules are the same: members mustn't
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
- Limits are unlimited unless configured. Never hardcode a usage cap; that
  includes picture uploads (a size per picture and bytes per account per day),
  which admins can turn on in instance settings.
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
- CI runs only the jobs a change can affect, and Publish image builds and
  redeploys fuwa.chat only when something the image is built from changed
  since the last image it published. `.github/scripts/changes.sh` decides
  both: a job is skipped only when every changed file is on its list of files
  that can't affect it, so a new folder runs everything until it's listed
  there. A pull request that changes what the Dockerfile builds with (the
  Dockerfile, a manifest, the web build's config) also builds the image in
  CI. Require the "CI passed" check, not the jobs under it.
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
- Secure channels (`api/secure.rs`, `docs/secure-channels.md`) hold to the
  same: the server keeps their MLS records in the server's file and checks
  only headers and permissions. Nothing that reads content (AutoMod, search,
  webhooks, agents, link previews, embeds) may touch them; their plaintext is
  a `DirectMessageContent`. Who belongs in a channel's group is
  `secure_members` (who can see it, people only); keep it in step with any
  change to how channel access is worked out.
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
  `cargo test`, and `buf lint`; for `web/`, `pnpm wasm`, `pnpm build` and `pnpm test`
  (node's own test runner over `src/**/*.test.ts`), then
  `cargo test --features web`; for `desktop/`, the same three cargo commands
  run inside `desktop/` (`cargo fmt`, not `--all`).
- The desktop app and the web app are two faces of one client: a feature,
  setting or fix in one belongs in the other too, and the desktop's core
  mirrors `web/src/fuwa` and `web/src/e2ee` file for file where it can. Its
  settings are this computer's and apply to every instance, like
  `web/src/lib/prefs.ts`. Pictures in messages show as links, as on the web.
