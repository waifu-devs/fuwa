# End-to-end encrypted direct messages

Direct messages in fuwa are end-to-end encrypted, always. Only the devices of
the two people in a conversation can read it. The instance stores and passes
along ciphertext; it never holds a key that opens a message, so neither it nor
whoever runs it (Waifu Devs on fuwa.chat, or a self-hoster) can read what was
said. There's no switch to turn it off, per conversation or anywhere else.

## The protocol: MLS

Conversations use **MLS, the Messaging Layer Security protocol
([RFC 9420](https://www.rfc-editor.org/rfc/rfc9420))**, through
[OpenMLS](https://github.com/openmls/openmls), its Rust implementation. MLS
is an IETF standard that was designed and analyzed in the open for years
before it was published. We picked it over rolling anything of our own, and
over Signal's double ratchet or Matrix's Olm/Megolm, because:

- **Devices are members.** An MLS group holds every device of everyone in it,
  so a person signed in on a laptop, a phone and the desktop app reads the
  same conversation on all three, each with its own keys. Adding and removing
  devices is part of the protocol, not a layer on top.
- **It grows into groups.** The same machinery runs group DMs and, one day,
  encrypted channels, with costs that grow with the log of the group's size.
- **Forward secrecy and post-compromise security.** Every message uses a key
  that's deleted once used, and every commit moves the group to a new epoch
  with fresh secrets, so a key stolen today doesn't open yesterday's messages,
  and one that was stolen stops working once the group moves on.
- **One library everywhere.** `fuwa-e2ee` (Rust) is the same code in the
  server (which only reads headers), the web app (as WebAssembly) and the
  desktop app (`desktop/`, natively).

Every conversation uses one cipher suite,
`MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519` (0x0003): X25519 for
key exchange, ChaCha20-Poly1305 for messages (fast in WebAssembly, where
there's no AES hardware), SHA-256, and Ed25519 signatures. Messages are padded
to a multiple of 64 bytes so their length says less about what's in them.

## The pieces

### Devices

Each browser (or app) signed in to an account is a device with its own Ed25519
signing key, made on the device and never sent anywhere. Its id is the first
16 bytes of the SHA-256 of the public key, as hex, so the server can't pass
one device's id off with another device's key. Its MLS credential is a basic
credential whose identity is the account's id: every message says which
person, and which of their devices, sent it.

A device belongs to a session: registering it (`RegisterDevice`) ties the key
to the session it's signed in with, and signing out, an admin ending the
session, or the session expiring takes the device away (an hourly sweep clears
any left behind). A session has one device; registering a new key replaces
the old one.

Each device publishes key packages (`AddKeyPackages`): the public half of
one-time keys others use to add it to a conversation or a secure channel.
Anyone who shares a server with it, or has a conversation with it, can claim
them. Each is handed out
once (`ClaimKeyPackages`); when a device runs out, its last-resort key package
is handed out instead until the device tops up. Someone with no conversation
with you gets at most 3 single-use ones from each of your devices an hour
(and 2,000 in all, adding people to secure channels), and last-resort ones
after that. The server checks that a key
package names the account and device it's published for.

### Conversations

A conversation is between two people who share a server (once it exists,
either can write in it even if they stop sharing one). It's an MLS group whose
id is the conversation's id and whose members are every device of both
people.

The instance keeps each conversation's **records** in one order: commits
(devices coming in or going) and encrypted messages. It takes a record only
for the group's current **epoch**, read from the record's MLS header, and a
commit moves the epoch on. That one rule keeps every device on the same
history: two devices that commit at once can't fork the group, because the
second commit is for an epoch that's gone, and its device catches up and
tries again. The instance also keeps the group info the last commit left
(public state), so a device can join by itself.

### Before sending

Before a device writes, it makes the group hold exactly the devices both
people are signed in on now: it asks the instance for their devices, claims a
key package for each new one, removes signed-out ones, and commits that. A
device added this way gets a welcome; the next time it looks, it joins from
it. A device nobody added yet (you just signed in somewhere) joins by itself
with an external commit from the group info.

A device only ever lets in devices of the conversation's two people: it
refuses a welcome, a group info or a commit that would bring in anyone else.
The instance can't add a device of a third person, and if it tried to add a
device of its own making to one of the two accounts, the safety number would
change.

### Safety numbers

The only thing a client can't check by itself is that the devices claiming to
be the other person's really are theirs. That's what the **safety number** is
for: 60 digits worked out from each person's id and the signing keys of all
their devices in the conversation (Signal's scheme: 5,200 rounds of SHA-512
per person, 30 digits each). Both people see the same number. If it matches
when they compare it, in person or on a call, nobody slipped a device in
between them. Marking it verified is remembered on the device; if it changes
later (someone signed in somewhere new, or something else happened), the
conversation shows that it changed, and they can compare again.

## What the instance sees

It can't see what anyone wrote, edited, or replied to, or what any device's
private keys are. It does see, and has to, to deliver:

- who has a conversation with whom, and when it was opened;
- when each record was sent, by which account and device, and roughly how big
  it was (padded);
- which devices each account has (a label from the browser, like "Chrome on
  Windows", and their public keys);
- which records are commits and which are messages, and when one is deleted;
- which messages carry a sealed file (a voice message), and that file's size,
  padded to 32 KiB steps: roughly how long it is, to within about eight
  seconds.

Deleting a message removes its ciphertext from the instance and leaves a gap;
copies already on devices are removed from the screen when they see the
deletion. Edits are new encrypted messages that point at the one they change.

A split instance's replica (`docs/storage.md`) backs `dms.db` up like every
other database, so the bucket holds the same ciphertext and nothing more. A
deleted message's ciphertext can stay in the replica's older log until that
generation is replaced.

## On the device

What a device has read, it keeps: MLS can't decrypt a message twice (its key
is deleted once used), so the device's copy is the only one. The web app keeps
its device state and its conversations in IndexedDB, under the instance and
account, and wipes them when it signs out, when the session ends, when the
instance is forgotten, or when the account is deleted. One tab works at a time
and tells the others (Web Locks and a BroadcastChannel).

A device can't read messages sent before it joined a conversation. A new
browser starts from when it signs in.

## Where it lives

- `e2ee/`: `fuwa-e2ee`. `wire.rs` reads MLS headers for the server;
  `device.rs` (feature `client`) is a whole device; `lib.rs` has device ids and
  safety numbers.
- `e2ee-wasm/`: the same for the web app, built with `pnpm wasm`.
- `proto/fuwa/v1/dm.proto`: `DirectMessageService`, and
  `DirectMessageContent`, the plaintext inside the encryption.
- `server/src/dms.rs`, `server/src/api/dms.rs`: `dms.db` and the service.
- `web/src/e2ee/`, `web/src/fuwa/dms.ts`, `web/src/components/dm/`: the web
  app's device, vault and screens.

## Voice messages

A voice message is recorded on the device (the browser's Opus encoder,
written as an Ogg Opus file by the app itself) and **sealed** there: a new
random AES-256-GCM key for that file alone, the sealed bytes being the
12-byte nonce, then the ciphertext and its tag. The device uploads the
sealed bytes (`DirectMessageService.CreateSealedUpload`, then a PUT like a
picture's), and sends an ordinary encrypted message whose content
(`DirectMessageVoice`) holds the file's id, its key, the SHA-256 of the
sealed bytes, how long it plays and its waveform (worked out on the sender's
device). Before sealing, the file is padded (a 0x80 byte, then zeros) to a
multiple of 32 KiB. So the instance keeps bytes it can't open and never
learns the key, the shape of the sound or its exact length (the padded size
gives it away to within about eight seconds); nobody else, transcription
services included, ever gets it.

`PostMessage` names the file in `media_ids`, which ties it to that record
(dms.db's `record_media`): one file, one message, and deleting the message
deletes the file. A file never sent is swept after a day like any unused
upload. Devices fetch it from their own instance by id (never from a link
in the message), check its size and hash, then open it; the player keeps
the opened sound in memory only. Admins can cap how long
(`voice_message_seconds`, which apps honour, since the instance can't check)
and how big (`voice_message_bytes`, checked on the sealed size) one may be,
and how many bytes of them an account may upload a day
(`voice_message_bytes_per_day`, counted apart from pictures, in dms.db's
`sealed_days`); none is capped unless they set it. A file is kept from the
sweep only once the message carrying it is in, so a send that fails leaves
nothing behind for good; the app sends it again with the same file. The server side is
`server/src/sealed.rs`; the web app's is `web/src/voice/` and
`web/src/components/voice/`, written so a server channel's composer and
messages can use the same recorder and player.

## Calls

Calls in direct messages are end-to-end encrypted with the same groups: each
device exports a secret from the conversation's group at its current epoch
(`Device::export_secret`, label `fuwa call v1`) and seals every frame of
sound with a key derived from it. See [calls.md](calls.md#direct-messages-are-end-to-end-encrypted).

## Secure channels

Channels in community servers can be end-to-end encrypted with the same
devices and the same kind of group: one MLS group per channel, holding the
devices of everyone the channel's permissions let see it. See
[secure-channels.md](secure-channels.md).

## Not done yet

- **Account keys.** Devices are trusted as the account's because the instance
  says so, and the safety number is how people check. Cross-signing (a device
  you already trust vouching for a new one) would let a new device in without
  changing what you need to compare.
- **History on new devices.** A backup of message keys, encrypted with a key
  only you hold, would let a new device read what came before it.
- **Group DMs**, search (it can only ever happen on the device), and
  attachments other than voice messages (sealed files are the way in).
