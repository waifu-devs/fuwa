# Changing keys

An instance holds up to three secrets of its own: the **encryption key**
(`FUWA_ENCRYPTION_KEY`, everything kept at rest), the **cluster key**
(`FUWA_CLUSTER_KEY`, every call between the parts of a split instance) and
the **media key** (`FUWA_MEDIA_KEY`, calls to media parts on hosts of their
own). Each can be changed, for instance after one leaks, without losing data
and, on a split instance, without stopping it: every part is given the new
key and told the old one as *previous*, the instance moves over, and then the
old one is taken away.

| Key | Previous keys | What happens while both are set |
| --- | --- | --- |
| `FUWA_ENCRYPTION_KEY` | `FUWA_ENCRYPTION_KEY_PREVIOUS` | Each database under an old key is copied under the new one as it opens; recordings are sealed again in the background, the replica's copies too |
| `FUWA_CLUSTER_KEY` | `FUWA_CLUSTER_KEY_PREVIOUS` | Parts take calls carrying either; links and tokens signed with the old key still check out |
| `FUWA_MEDIA_KEY` | `FUWA_MEDIA_KEY_PREVIOUS` | Media parts take calls carrying either |

Each `_PREVIOUS` is a comma-separated list, normally of one key. Keys are
never logged; the log only says how many previous keys a part has.

## The encryption key

What it covers: node.db, dms.db and every server's file (Turso's AEGIS-256
encryption), call recordings (ChaCha20-Poly1305 under a key derived from it),
and so everything the replica copies to its bucket. Uploaded pictures and
files were never encrypted by it.

1. Make a new key: `openssl rand -hex 32`.
2. On every part that keeps data (a single process, or the directory and every
   shard), set `FUWA_ENCRYPTION_KEY` to the new key and
   `FUWA_ENCRYPTION_KEY_PREVIOUS` to the old one, and restart them (one at a
   time is fine).
3. As each opens its databases, those under the old key are copied under the
   new one and replace the old files (`db::rekey`): `encrypted a database under
   the new key` in the log, once per file. A big file takes about as long as
   reading it. Recordings are sealed again after startup, and the log says
   `every recording is sealed under the encryption key` when it's done.
4. Once every part has said so, remove `FUWA_ENCRYPTION_KEY_PREVIOUS` and
   restart them again.

`FUWA_ENCRYPTION_KEY_PREVIOUS=none` encrypts an instance that ran without a
key: its plain files are copied under `FUWA_ENCRYPTION_KEY` the same way.

A file under a key that isn't listed still won't open, as before: the log
names `FUWA_ENCRYPTION_KEY`.

### Server files moving between shards, and the replica

Server files can arrive from another shard (a move, a hand-off) before that
shard has the new key. A shard with the old key as previous opens them anyway
and encrypts them under the new one, so the parts can change over in any
order, as long as each has both keys until all have.

The replica copies what's on disk, so a file encrypted again starts a new
generation in the bucket (a fresh snapshot, see [storage.md](storage.md)).
The generation before it, under the old key, is kept until the next one
replaces it, as usual. `fuwa restore` takes either: a snapshot under a key in
`FUWA_ENCRYPTION_KEY_PREVIOUS` is replayed under it and then encrypted under
the new key. A restore with only the new key can't read a generation from
before the change.

After a leak, the bucket's old generations are still readable with the old
key until they're replaced. To be rid of them sooner, let the parts run with
the new key until each file has started a new generation (they do whenever
the logs shipped since outgrow the file), or empty the bucket's old
generations by hand.

### Recordings

Each sealed recording file is checked by its first chunk. One under a previous
key is read with it and written again under the new key next to it, then put
in its place; the replica's copy is replaced too, and copies only in the
replica (a shard that took the server over) are fetched, sealed again and put
back. Until that's done, downloads read old files with whichever key they're
under. A file that unseals with no key the instance has is left alone and
counted in the warning.

## The cluster key

Every call between parts carries the cluster key, and parts answer only calls
that do (`cluster::require_key`). On a split instance the key also signs
picture links (`/media/outside/...`), single sign-on state, GIF search results
and links to files shared from other instances (`outside::Key`, derived from
the cluster key so every part has the same).

1. Make a new key: `openssl rand -hex 32`.
2. On every part, set `FUWA_CLUSTER_KEY` to the new key and
   `FUWA_CLUSTER_KEY_PREVIOUS` to the old one, and redeploy one part at a time
   as usual (fuwa.chat's rollout does). Calls keep flowing throughout.
3. Once every part runs with both, remove `FUWA_CLUSTER_KEY_PREVIOUS` and
   redeploy again.

How the two overlap: a part sends every key it has on each call, as separate
values of the `x-fuwa-cluster-key` header, the previous ones first
(`cluster::SentKeys`). A part from before the change reads only the first
value, which is its own key; a part that has moved on takes a call carrying
any key it knows (`cluster::AcceptedKeys`). So any two parts can talk at every
step: old and new during step 2, new with and without the previous key during
step 3.

While a part has the old key as previous, links and tokens signed with it
still check out, and everything new is signed with the new key. A part not
yet given the new key doesn't take what the new key signed, so for the minutes
of a rollout a picture link or sign-in made on an updated part may fail on one
that isn't yet; it works again once both are updated.

### Old picture links

Picture links are stored with what they're in (embeds, webhook avatars, link
previews), signed when stored. Once the old key is gone, links signed with it
no longer load. To keep them loading after the change, move the old key to
`FUWA_CLUSTER_KEY_RETIRED` in step 3 instead of dropping it: retired keys
check picture links and nothing else, never internal calls, sign-ins or other
tokens. Someone holding a retired key can only have the instance fetch public
pictures for them, as any picture link does (public addresses, pictures,
8 MB at most); leave it out if that matters more than the old pictures.

A single process has no cluster key: its links are signed with a key kept in
node.db.

## The media key

A media part on a host of its own takes only the media key
([self-hosting.md](self-hosting.md#the-media-part-on-a-host-of-its-own)).

1. Make a new key.
2. Set `FUWA_MEDIA_KEY` to it and `FUWA_MEDIA_KEY_PREVIOUS` to the old one on
   the media part and on the directory and every shard (the parts that call
   it), in any order. Calls in progress ride each restart out.
3. Remove `FUWA_MEDIA_KEY_PREVIOUS` everywhere.

The same steps give media parts a key of their own where they used the
cluster key: set the new `FUWA_MEDIA_KEY` with
`FUWA_MEDIA_KEY_PREVIOUS` set to the cluster key, then remove it. A media part
without a media key takes the cluster key and both lists of previous keys.

## After a leak

- **Encryption key**: change it as above. The files on disk are re-encrypted;
  copies made before (the replica's older generation, backups, saved server
  files) are still under the old key, and saved server files aren't
  encrypted at all.
- **Cluster key**: change it as above, and drop the old key at step 3 rather
  than retiring it. Whoever had it could call any part directly, so check
  the instance's accounts and admins too.
- **Media key**: change it as above. Whoever had it could open and close calls
  on the media part, not read anything kept.

Sessions, agents' tokens, two-step secrets and the instance's federation key
are separate: an admin turning an account off ends its sessions, a new agent
token replaces the old, and federation keys change in the instance's settings.
