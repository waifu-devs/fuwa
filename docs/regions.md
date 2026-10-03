# Regions

A split fuwa instance (see [Scaling out](../README.md#scaling-out)) can run
its parts in more than one part of the world: shards and media parts in the
US, in Europe, in Asia, and so on. Each community server lives in one region,
chosen when it's made, and everything that server holds stays there. That's
what makes an instance usable by communities that need their data kept in
the EU (GDPR) or near their members, and it's how the gateways know which
shards to send a server's calls to.

An instance run as one process (`FUWA_ROLE=all`, what most self-hosters run)
is one region by definition and nothing here changes for it.

## Regions are labels

A region is a short label, `FUWA_REGION`, such as `us-east`, `us-west`, `eu`
or `asia` (1 to 32 of a-z, 0-9 and `-`). Every part can carry one:

- the **directory**'s region is the instance's **home region**: where
  accounts, sessions, settings and direct messages are kept (see
  [What stays in the home region](#what-stays-in-the-home-region));
- each **shard** says which region it's in when it registers, and the
  directory keeps it with the shard (node.db, `shards.region`);
- **media** parts and **gateways** carry one so logs and the operator can
  tell them apart; the shards of a region are pointed at that region's media
  parts (`FUWA_MEDIA_URL`), and the IaC does that pairing.

A part without `FUWA_REGION` is in the home region, so an instance that has
never set it (fuwa.chat today) is one region and keeps working as it does.
`FUWA_REGION_NAME` gives a region the name people see ("Europe"); common
labels have one built in (`eu` is "Europe", `us-west` is "US West").

Regions are never guessed from anyone's address. fuwa doesn't look up where
a person is, and nothing here reads or keeps a client's IP address.

## A server's region

- **Chosen when it's made.** `CreateServer` takes an optional `region`. The
  apps show a region picker in the "Create a server" dialog when the instance
  has more than one region, defaulting to the home region. The directory puts
  the new server on the shard with the fewest servers among the shards of
  that region that are up; with none up there, making it fails rather than
  landing somewhere else.
- **It's the region of the shard holding it.** A server's file says nothing
  about regions: its region is where its shard is, so moving the file is
  moving the server. Shards stamp their region on every `Server` they answer
  with, so apps can show it (a badge in the server's settings) and the
  directory's index has it.
- **Listed by the instance.** `GetNode` lists the regions that have a shard,
  with the home region first, so apps can offer them.

## Routing

Nothing changes in how calls find their shard: the gateway reads the
`server_id` in each server-scoped call, asks the directory where it is
(remembered until the shard moves or goes down) and passes the call there.
With regions, "there" may be another continent. Gateways keep nothing, so
they can run in any region, and as many regions as wanted; a call to an EU
server passes through whichever gateway the client reached, held only in
memory while it's on its way and never logged beyond its path.

Live event streams already merge one stream per shard, so a person in servers
in three regions gets one stream fed by three regions' shards.

Later, so a client can reach a region's gateways directly: each region gets
its own public address (`eu.fuwa.chat`), `GetNode` lists it, and apps send a
server's calls to its region's address. That keeps even the transit inside
the region and cuts the round trip. It isn't in this release.

## Calls

A voice channel's call is kept by the shard holding its server, which opens
it on one of its region's media parts. People's sound and video go straight
between their app and that media part, so a call in an EU server is carried
in the EU. Calls in direct messages are kept by the directory and carried by
the home region's media parts; they're end-to-end encrypted, so those parts
only ever see sealed frames.

Server-side recordings are written by the server's shard, onto its volume,
and copied to that shard's replica bucket.

## Moving a server to another region

An instance admin can move a server (`AdminService.MoveServer`, in the
instance's Servers page). The directory runs the move:

1. It picks a shard in the new region (the one holding the fewest servers)
   and records the move in node.db (`moves`), so a restart in the middle
   finishes or undoes it rather than leaving the server in two places.
2. The new shard asks the old one for the server. The old shard stops taking
   changes to it (reads go on), folds its log into the file, ends any
   recording and call in it, and sends the file and its recordings, each
   checked by SHA-256.
3. The new shard opens the server, starts replicating it to its own bucket,
   and says it has it. The directory points the server at the new shard.
4. The old shard lets go: requests for the server are sent on to the new
   shard, open live streams follow it there from where they were, and the old
   shard deletes its files, its recordings and its replica's copies (only
   those, if both shards share a bucket).

Changes made during the move wait at the gateway (the same way they wait out
a deploy, up to 30 seconds) and then go through on the new shard, so for most
servers the move is a pause of a second or two. People in the server's calls
are disconnected and rejoin on the new region's media part, as after a
restart. A move that fails before step 3 leaves the server where it was.

## What stays in the home region

Some things belong to a person or to the instance, not to one server, and
live with the directory:

- **Accounts** (profile, sign-in, sessions, two-step secrets), **notification
  settings**, **agents** and instance **settings**: node.db.
- **Direct messages**: dms.db, end-to-end encrypted, so the directory only
  holds ciphertext, key packages and which devices are in each conversation.
- **Uploaded pictures**: avatars, banners and backgrounds, and today also
  server icons, custom emoji and webhook pictures, in `media/` with a row in
  node.db; pictures from other sites fetched for links and embeds are cached
  there too.
- The directory's **index** of every server: its profile (name, icon,
  description), its members' ids and its invite codes, in memory, rebuilt
  from the shards; and node.db's `placements`.

That makes the home region where an instance's operator should be: an
instance run for EU communities puts its directory in the EU, and every
region's personal data that isn't a server's content stays there too.

Server icons, emoji and webhook pictures belong to a server and are the one
part of a server's content that doesn't follow it yet. Moving them to the
server's shard (uploads routed by `server_id`, served by the shard) is the
next step after this one; until then the docs and the move dialog say so.

Server **attachments** are links today (fuwa doesn't store uploaded files for
messages), so they're kept wherever they're hosted. When fuwa stores
attachments, they'll go to the server's shard and its region's bucket.

## GDPR: what fuwa does, and what's left to the operator

What fuwa does:

- A server's content (its messages, channels, members' copies of profiles,
  roles, audit log, recordings, and its calls' media) is stored and carried
  only by its region's shard, media parts and replica bucket. Moving it
  deletes the old region's copies.
- Personal data that isn't a server's lives in the home region, in one place.
- No IP addresses are logged, stored or sent anywhere (see the security notes
  in [AGENTS.md](../AGENTS.md)); rate limits count per account or server.
- Telemetry is aggregate: the anonymous usage signal and the anonymous
  reports carry counts and timings, no ids, names, text or addresses, and the
  operator can turn them off.
- People can export their data (`ExportAccount`, gathered from every region's
  shards into a file for them) and delete their account (taken out of every
  server in every region; it waits until every shard is up).

What the operator must still do:

- Have processor terms (a DPA, and standard contractual clauses for transfers
  out of the EU) with the hosting provider for every region used, Railway for
  fuwa.chat, and with the bucket provider.
- Say in their privacy policy which regions exist, what the home region
  holds, and that calls through a gateway in another region pass through it
  (or run gateways in the region, and later regional addresses).
- Answer requests that aren't self-service: erasure of a server's content
  (deleting the server, then emptying `deleted/` on its shard and the bucket's
  copies after the retention they choose), access requests by people without
  an account, and records of processing.
- Put the directory in the region whose rules the instance follows, and keep
  backups (Railway volume backups, buckets) in the same region as what they
  copy.

## Running it

On each shard and media part, set `FUWA_REGION` (and on the directory, the
home region's label). Point each shard at its region's media parts with
`FUWA_MEDIA_URL` and at its region's bucket with `FUWA_S3_*`. Shards in
different regions need the same `FUWA_ENCRYPTION_KEY` and `FUWA_CLUSTER_KEY`
as the rest of the instance, and reach the directory over a private network
(Railway's spans regions within a project).

For fuwa.chat, `.railway/railway.ts` lists regions as data (`REGIONS`): each
with its Railway region, bucket region, number of shards and whether it has
gateways. The home region is the one it runs in today; adding a region adds
its shards, volumes, media part and bucket. Railway can't move an existing
service between regions, so a region is added, never moved.
