# Storage past one volume

Today every fuwa database is a file on one disk: `node.db` plus one
`servers/<id>.db` per community server, each with the `.db-log` Turso keeps
beside it. On Railway that disk is a volume, and a volume sets three limits at
once:

- **Size.** 5 GB on the Hobby plan, 50 GB by default on Pro, at most 1 TB. A
  volume can grow but not shrink.
- **Safety.** It's the only copy. Railway's volume backups help (see
  [A stopgap](#a-stopgap-railways-volume-backups)), but they live in the same
  project and go when the volume does.
- **Deploys.** A service with a volume can't run two deployments at once, so
  every deploy of a part that keeps files drops its connections for a moment.

This document is the plan for lifting those limits, and the one place that
describes how fuwa stores data past a volume. It starts with this release:
every database and picture is copied, as it changes, to an S3-compatible
bucket (a Railway bucket for fuwa.chat), and the instance can come back from
that copy. The later phases build on it until the volume is a cache of the
servers in use rather than the place everything lives.

It's for an instance split into parts (`FUWA_ROLE=directory` and `shard`,
see [Scaling out](../README.md#scaling-out)), which is how fuwa grows past one
machine. An instance run as one process (`FUWA_ROLE=all`, the default, and
what most self-hosters run) keeps plain local files: it refuses the replica's
settings, and its backups stay a copy of the data folder.

## What Turso offers, and why fuwa ships its own log

fuwa runs every file in Turso's concurrent-writer mode (MVCC). Checked against
turso 0.7.2 (what fuwa uses) and 0.8.1 (the newest, out in late September 2026):

| Turso feature | What it does | Fits fuwa? |
| --- | --- | --- |
| The MVCC log (`<file>.db-log`) | Every commit is appended to it as a frame; the file itself only changes when the log is folded in (a checkpoint). Frames are encrypted with the database's key. | **Yes: this is what fuwa ships.** It's append-only between checkpoints, and replaying it is Turso's own crash recovery. |
| Sync (`turso_sync`, push and pull) | Keeps a local file in sync with a remote database, sending changes as SQL. | No. It talks to Turso Cloud (or `tursodb --sync-server`, a test server), and it doesn't support local encryption at rest, which fuwa.chat uses. |
| Turso Cloud | Hosted databases, stored on object storage behind the scenes. | Not as the base. It's closed source, so a self-hosted instance couldn't do the same, and it would hold every community's data outside our infrastructure. It could be an optional place to replicate to later. |
| Bottomless (libSQL) | Ships the WAL of libSQL, the C fork of SQLite, to S3. | No. It's for libSQL, not for Turso's Rust engine or its MVCC log. |
| Litestream, LiteFS | Ship SQLite's WAL frames, relying on SQLite's file locks. | No. In MVCC mode commits don't go through the WAL the way they expect. |
| `VACUUM INTO` | Writes a copy of the database. | Not for backups: from an encrypted file it writes an unencrypted copy. |
| `DurableStorage` (in `turso_core`, 0.8) | A hook that sees every committed transaction and every checkpoint, meant for exactly this kind of replication. | Later. The `turso` crate fuwa builds on doesn't expose it yet. Phase 4 switches to it from reading the log file. |

So the replica copies Turso's own log, byte for byte, and lets Turso replay it
on restore. fuwa never decodes the log: it relies on two facts about it, that
it only grows between checkpoints and that its header carries a salt that
changes whenever it starts over, and on Turso to replay it.

## How the replica works (phase 1, in this release)

### What goes where

```
<bucket>/<FUWA_S3_PREFIX>/
  node/current                      "<generation> <writer>": which generation is in use, and who wrote it
  node/<generation>/snapshot        node.db as the generation began
  node/<generation>/<epoch>-<offset>  bytes of the log, from that offset, in that epoch
  dms/…                             dms.db (direct messages, end-to-end encrypted), laid out like node/
  servers/<id>/current              "<generation> <writer> <shard>": the same, and the shard holding it
  servers/<id>/<generation>/…
  servers/<id>/deleted              set when the server is deleted; restores leave it out
  media/<id>                        each uploaded picture, as it is on disk
```

The directory replicates `node.db`, `dms.db` and the pictures; each shard, the servers
it holds. Servers are kept by their id, not under the shard holding them, so a
server keeps its history wherever it moves; its `current` names the shard that
wrote it last.

### Shipping

For each file:

1. **A generation starts** with a copy of the file. Turso's own automatic
   checkpoint is turned off for the file, so after one checkpoint the file
   doesn't change while it's copied; writes carry on into the log.
2. **Every `FUWA_REPLICA_INTERVAL`** (default 1 second) the bytes the log
   gained since the last round go up as one object (a segment). A file nothing
   wrote to costs a `stat`.
3. **When the log reaches 4.12 MB** (Turso's own threshold), the replica folds
   it in itself. It holds writes for that moment, reads the log's last bytes,
   folds the log into the file, lets writes go, then uploads those bytes. The
   log then starts over as the next *epoch* of the same generation. Turso
   holds writes while it folds the log too; the difference is who decides
   when.
4. **When the logs shipped since the snapshot outgrow the file** (and are at
   least 64 MiB), a new generation starts with a fresh copy. The current
   generation and the one before it are kept; older ones are deleted.

Writes go through one gate per file (`Db::shared` and `Db::alone` in
`server/src/db.rs`). Ordinary writes share it; folding the log takes it alone,
so no commit can land between reading the log's last bytes and folding it.

The bucket only ever holds what's on disk. With `FUWA_ENCRYPTION_KEY` set
that's ciphertext: Turso encrypts database pages and log frames alike, so the
bucket's owner can't read messages without the key. What's visible is the
shape: file sizes, when commits happened, how big they were. Pictures aren't
encrypted on disk, so they aren't in the bucket either.

### Restarts, crashes and two writers

Where each file's replica stands is kept in `<data>/replica/`, so a restart
carries on the same generation. On start each file is checked against it (the
file's size and modified time, the log's salt and its last bytes); if anything
moved on that the replica didn't see (a crash between folding the log and
uploading its tail, a file swapped by hand), that file starts a new generation.
Nothing is ever lost to that: the new snapshot has everything.

Each data directory has a random writer id, written into every `current` it
sets. If a second process starts replicating the same file to the same bucket
(two instances pointed at one prefix, or the same server opened on two
shards), the first one notices within a minute, logs an error and stops
writing that file's replica rather than mixing two histories.

### Restoring

With `FUWA_RESTORE=if-empty`, each part restores itself at start when its
data directory is empty (a new or wiped volume):

- the **directory**, when it has no `node.db`: `node.db`, `dms.db` and the pictures;
- a **shard**, when its `servers/` has no server files: every server whose
  `current` names it (by `FUWA_SHARD_ID`), leaving out deleted ones. A shard
  whose name was made up on its first start loses that name with its volume,
  so give each shard a `FUWA_SHARD_ID` (the Railway layout does).

Each file comes back by downloading its snapshot and replaying each epoch's
log in order, letting Turso fold each one in, then opening it once more to
check it. A file is built next to its place and moved in only when it's whole.

Without `FUWA_RESTORE`, a part whose data directory is empty while the replica
holds its files refuses to start, so a lost volume never quietly turns into an
empty instance writing over its own backup.

By hand, with the part stopped: `fuwa restore [DIR]` restores what the part
`FUWA_ROLE` names keeps, `fuwa restore --server <id> [DIR]` restores one
community server (to move it to another shard), and `fuwa restore --list`
shows what the replica holds, and which shard holds each server.

### What a failure loses

The replica is asynchronous. If the volume vanishes mid-flight, what was
committed since the last round (at most `FUWA_REPLICA_INTERVAL`, plus however
long the upload took) is lost. A clean shutdown (a deploy, a restart) ships
everything first. If the bucket is down, the instance keeps running, logs the
failure, and catches up when it's back. Logs aren't folded meanwhile, so they
grow on disk, up to 16 times Turso's threshold (about 66 MB per file); past
that a file's log is folded in anyway, and its replica starts a new generation
once the bucket is back. A file whose replica another process took over is
folded as usual.

## What it buys

- **Durability.** Losing or wiping the volume no longer loses the instance.
- **Restore anywhere.** A new volume, another region, a laptop: start the
  part pointed at the bucket with `FUWA_RESTORE=if-empty`.
- **Moving a server between shards** stops being a file copy between volumes:
  the new shard restores it from the bucket (by hand in phase 1, with
  `fuwa restore --server`; by itself in phase 2).
- **The volume stops being the ceiling** in phase 3, when servers nobody is
  using leave the volume and live only in the bucket. Phase 1 is what makes
  that safe: every server is already there.
- **Deploys without a drop** for shards, in phase 2: a new shard can restore
  its servers from the bucket and take them over, instead of waiting for the
  old one to let go of a volume.

## What it costs

- **Bucket storage**: $0.015 per GB-month on Railway. Each file keeps up to
  two generations, each a snapshot plus at most as much log as the snapshot's
  size (or 64 MiB), so the bucket holds about two to four times the data.
- **Upload traffic**: uploads from a service count as its egress, $0.05 per
  GB. Every byte a commit writes to the log goes up once, plus a fresh snapshot
  of each file every time its logs outgrow it. Bucket requests and reading from
  the bucket are free on Railway.
- **Requests**: one upload per changed file per interval, and one read of each
  file's `current` a minute. Idle files make none.
- **Writes pause while the log is folded**, as they do today when Turso folds
  it; the pause now also covers reading the log's last few bytes.
- **Restore time** grows with the data: a snapshot and up to a generation's
  logs per file. Fine for the servers fuwa.chat has; phase 3 restores servers
  only when they're opened.
- **Disk while restoring**: files are rebuilt on the same volume before they
  move into place.
- **A file over 5 GB** can't go up in one request; multipart uploads come when
  a server gets near that.
- **Turso's log format.** The replica relies on the log being append-only
  between checkpoints and on where its salt sits. A Turso upgrade has to keep
  the tests in `server/src/replica/tests.rs` passing, which check exactly that.

## What changes for shards

- **Now (phase 1).** Every part that keeps files replicates its own: the
  directory `node.db`, `dms.db` and the pictures, each shard the servers it holds. Give
  them all the same bucket and prefix; the keys don't overlap, and gateways,
  which keep nothing, ignore the settings. A part that loses its volume comes
  back with `FUWA_RESTORE=if-empty`, the shards by their `FUWA_SHARD_ID`.
- **With the Railway layout** (`SPLIT` in `.railway/railway.ts`: volume-less
  gateways, a `fuwa-directory` service on today's volume, and
  `fuwa-shard-<n>` services with a volume each and a fixed `FUWA_SHARD_ID`),
  nothing about the split changes. The first shard to start still takes the
  single process's server files from the directory (`TakeServers`, each file
  checked before the directory lets go), and only then checks the replica, so
  a shard that just took them never looks empty to it. Those files were never
  replicated, since a single process doesn't replicate, and they go up from
  that shard as soon as it opens them, each as a new generation under its
  name. From then
  on, a shard deploy that loses nothing on the volume restores nothing, and
  one on a fresh volume restores its servers from the bucket instead of
  starting empty. Moving a server by hand (stop both shards, move its files,
  start them) carries its history over too: the shard that opens it writes
  its `current` from then on.
- **Phase 2: the bucket says who holds a server.** A shard takes a lease on a
  server in the bucket (`servers/<id>/lease`, with conditional writes so only
  one shard can hold it) and renews it. A shard asked to open a server it
  doesn't have restores it from the bucket first. Moving a server becomes:
  the old shard ships its last commits and lets go, the new one takes the lease
  and restores. If a shard dies, the directory gives its servers to others once
  its leases run out, and they restore from the bucket, losing at most the
  replica interval. This is also what lets a shard deploy without a drop: the
  new deployment restores and takes over before the old one stops.
- **Phase 3: the volume is a cache.** A shard keeps the servers people are
  using on disk and lets the others go (ships their last commits, then deletes
  the local files). Opening one again restores it. The volume then needs to
  fit the servers in use, not all of them, and a shard's disk can be small
  and disposable.

## A stopgap: Railway's volume backups

Railway can back up a volume on a schedule (daily, weekly, monthly), kept
incrementally and restorable from the dashboard. It's zero code and worth
turning on for fuwa.chat now, alongside the replica. It doesn't replace it:
the backups stay in the same project and environment, are deleted with the
volume, can't be restored somewhere else, and are only as fresh as the last
one.

## The phases

1. **Continuous backup and restore** (this release), for split instances.
   Off unless `FUWA_S3_BUCKET` or `FUWA_REPLICA_PATH` is set on the directory
   and shards. For fuwa.chat, once `SPLIT` is on: a bucket in
   `.railway/railway.ts`, behind a switch of its own, with the variables
   below on the directory and every shard.
2. **Placement through the bucket**: leases with conditional writes, shards
   that restore servers they don't have, moving and failing over through the
   bucket, shard deploys that overlap.
3. **The volume as a cache**: cold servers live only in the bucket and come
   back when opened.
4. **Durable before it's acknowledged**, for the servers that want it: a commit
   returns once it's in the bucket, so a lost shard loses nothing. Built on
   Turso's `DurableStorage` hook (with the turso upgrade to 0.8), which also
   replaces reading the log file.

## Running it

| Variable | Default | What it does |
| --- | --- | --- |
| `FUWA_S3_BUCKET` | unset | The bucket to replicate to |
| `FUWA_S3_ENDPOINT` | AWS, from the region | The bucket's endpoint, like `https://t3.storageapi.dev` on Railway |
| `FUWA_S3_REGION` | `auto` | The bucket's region (`auto` on Railway, Cloudflare R2 and Tigris) |
| `FUWA_S3_ACCESS_KEY_ID`, `FUWA_S3_SECRET_ACCESS_KEY` | unset | The bucket's credentials |
| `FUWA_S3_PATH_STYLE` | `off` | `on` for services that want `endpoint/bucket/key` addresses (MinIO, Garage) |
| `FUWA_S3_PREFIX` | none | A folder in the bucket, to share one between instances |
| `FUWA_REPLICA_PATH` | unset | A folder to replicate to instead of a bucket (another disk, a network share) |
| `FUWA_REPLICA_INTERVAL` | `1s` | How often new commits go up (`50ms` to `60m`) |
| `FUWA_RESTORE` | `off` | `if-empty`: restore from the replica when the data directory is empty (no `node.db` on the directory, no server files on a shard) |

Set them on the directory and every shard. A gateway ignores them, and an
instance run as one process refuses them.

On Railway, a bucket gives the services the values to reference:

```ts
const replica = bucket("fuwa-replica", { region: "iad" });
// in the directory's and each shard's env:
FUWA_S3_BUCKET: ref(replica, "BUCKET"),
FUWA_S3_ENDPOINT: ref(replica, "ENDPOINT"),
FUWA_S3_ACCESS_KEY_ID: ref(replica, "ACCESS_KEY_ID"),
FUWA_S3_SECRET_ACCESS_KEY: ref(replica, "SECRET_ACCESS_KEY"),
FUWA_RESTORE: "if-empty",
```

The code lives in `server/src/replica/`: `s3.rs` (a small S3 client, signed
with AWS Signature V4), `store.rs` (a bucket or a folder), `mod.rs` (shipping
and restoring) and `tests.rs`.
