# Secure channels

A secure channel is a channel in a community server whose messages are
end-to-end encrypted, the way direct messages are ([e2ee.md](e2ee.md)). Only
the devices of the people who can see the channel can read what's said in it.
The instance keeps and passes along ciphertext; it never holds a key that opens
a message, so neither it nor whoever runs it can read the channel.

Anyone with Manage Channels makes one (channel type **Secure**). Who is in it
is decided like any private channel: its permissions. People who can see it
read it; people who can also send messages there write in it. A channel is
made secure, or not, when it's made, and stays that way: a channel that was
readable on the server can't become private after the fact, and a secure
channel's history can't be handed to the server.

## How it works

It's built from the same pieces as direct messages:

- **The protocol is MLS** ([RFC 9420](https://www.rfc-editor.org/rfc/rfc9420)),
  with the same cipher suite, library (`fuwa-e2ee`) and device keys. A device
  registered for direct messages is already ready for secure channels.
- **Each secure channel is one MLS group** whose group id is the channel's id
  and whose members are the signed-in devices of everyone who can see it.
- **The instance keeps each channel's records in one order** (commits that
  change which devices are in, and encrypted messages) in the server's own
  database file, on its shard. Like a conversation, it takes a record only for
  the group's current epoch, so two devices that commit at once never fork the
  group: the second one catches up and tries again. It keeps the group info
  the last commit left, so a device can join by itself, and the welcomes
  waiting for devices that were added.
- **What a message says is a `DirectMessageContent`** (text, an edit, a
  reply), inside the encryption.

### Keeping the group in step with the permissions

The server knows who can see the channel; only the devices can change the
group. So the apps keep the two in step:

- **Before a device writes**, it asks who can see the channel now
  (`GetSecureChannel`), lists those people's signed-in devices, adds the
  devices that aren't in the group (with key packages it claims for them) and
  removes everyone else's, and commits that. Then it sends. A message is never
  encrypted to someone the server says can't see the channel.
- **When permissions change** (a channel's overwrites, a role, someone's
  roles, someone leaving), every online device in the channel whose person
  may write there brings the group back in step after a short random wait.
  The first commit wins; the others catch up, find nothing left to do, and
  commit nothing.
- **Only people who may write commit.** A commit from a member of the group
  needs Send Messages: the server can't read a commit to check it's sound,
  and one no device can follow would break the channel for everyone. Someone
  who can only read joins by themselves and never changes the group.
- **The server stops showing the channel at once** to someone who loses
  access: they stop getting its records, live or listed, before any commit
  lands. Their devices are taken out by the next commit, after which what's
  sent can't be opened with anything they hold.
- **A device nobody added yet** (you signed in somewhere new, or you were just
  given access) joins by itself from the group info with an external commit.
  The server takes that only from someone who can see the channel.

The first device to write in a new channel starts its group, even if nobody
else is signed in anywhere yet; the others come in as their devices appear.

### Starting over

If the group still can't be followed (a commit or group info that no device
can read, which the server can't tell from a good one), anyone with Manage
Channels starts the channel's encryption over from its encryption panel, or
from the notice the channel shows when that happens (`ResetSecureChannel`).
The server puts the channel back at epoch 0 with no group info and no
welcomes waiting, and adds a reset record: every device forgets the group,
the channel shows a line saying who started it over, and the first device
that may write starts a new group and brings everyone back in. What devices
already read stays on them. Resets are in the audit log.

### What the server checks

Everything the server checks it can read without decrypting anything: the
caller can see the channel (and send there, for a message or a member's
commit; slow mode and time-outs apply as in any channel), a record's MLS header names this channel
and the current epoch, a commit carries the group info it leaves behind, and a
welcome goes only to signed-in devices of people who can see the channel.
Devices live where direct messages keep them (the directory), so a shard asks
the directory for them (`SecureDevices` in the cluster protocol).

Adding someone takes one of their key packages. Anyone who shares a server
with them can claim them, but an account gets at most 2,000 single-use key
packages an hour from people it has no conversation with, and at most 3
from any one of their devices; past that it gets their last-resort key
package, so nobody can use up someone's single-use ones on purpose.

### Who's who

Every device that's added or leaves shows as a line in the channel ("Juan
added Mika's device"), taken from the decrypted commit, not from anything the
server says. Someone whose devices you verified in a direct message (by
comparing safety numbers) shows as verified in the channel's encryption panel,
as long as every device of theirs in the channel was one you verified.

## What the server sees

It sees what it has to, to deliver and to apply the server's rules: who can
see the channel, who sent each record and from which device, when, roughly how
big it was (padded), whether it's a commit or a message, and when one is
deleted. The channel's name and topic are ordinary channel settings, not
encrypted.

## What doesn't work in a secure channel

The server can't read the messages, so nothing that needs to can work. The
channel says so once, at its top:

- **AutoMod** rules don't run here.
- **Search** can't find these messages (it could only ever happen on the
  device).
- **Bots, agents and webhooks** can't post or read: they have no device.
- **Link previews and inline pictures**: links stay links. The reader's app
  never fetches anything from other sites, and the server can't fetch what it
  can't read.
- **Mentions**: the server can't see who a message mentions, so it notifies
  nobody in particular; each device works out whether a message mentions you
  when it opens it, and chimes and notifies by the channel's notification
  settings.
- **Voice and server recordings**: secure channels are text.

Moderation that doesn't need to read still works: moderators (Manage
Messages) delete anyone's message there (its ciphertext goes, and the audit
log notes it), and time-outs, kicks and bans apply.

## History and devices

- **People who join later see only what's sent after they join.** That's what
  MLS gives: a new member gets the group's secrets from then on, never the old
  ones. Sharing older messages with someone, explicitly, is planned next: a
  member's device would bundle what it has, encrypt the bundle, and send its
  key to the new person's devices only (as Matrix's encrypted history sharing
  does).
- **A new device starts empty**, as with direct messages: MLS can't decrypt a
  message twice, so each device keeps what it read. A key backup only you can
  open would fix this for direct messages and channels together.
- **Losing access** takes the channel off the device too: the app forgets its
  group and what it kept of it.

## Size

A secure channel holds at most 500 people. Commits grow with the log of the
number of devices, but the group info a joining device downloads, and a commit
that adds many devices at once, grow with every device. Measured with
`fuwa-e2ee` (`cargo test -p fuwa-server --release --test secure -- --ignored`),
one commit adding every device at once:

| Devices | Group info | Welcome | Commit | Making it |
| ------: | ---------: | ------: | -----: | --------: |
| 10      | 1 KiB      | 2 KiB   | 2 KiB  | 3 ms      |
| 100     | 17 KiB     | 28 KiB  | 27 KiB | 34 ms     |
| 500     | 87 KiB     | 144 KiB | 137 KiB| 194 ms    |
| 1,000   | 173 KiB    | 289 KiB | 275 KiB| 529 ms    |

(Native, release build, on the machine that ran the tests; WebAssembly in a
browser is slower.) Later commits add or remove a few devices and stay small.

## Trust

MLS makes sure only the group's devices can read a message, and that every
device sees the same group. It can't decide who belongs: the server does, from
the channel's permissions. An operator who wanted to read a channel could
claim someone (or a device of their own making) can see it. Members would see
it happen: the device line in the channel, the person in the encryption panel,
and a changed safety number for anyone verified. Signed membership (an admin's
device vouching for each change) would close that gap and is a possible later
step. Discord's call encryption (DAVE) has its server propose changes that
clients commit; fuwa's clients commit by themselves, because the server never
handles MLS beyond reading headers.

## Where it lives

- `proto/fuwa/v1/secure.proto`: `SecureChannelService` (every call names its
  server in field 1, so a split instance routes it to the server's shard) and
  `SecureRecord`; `ChannelType.CHANNEL_TYPE_SECURE`; the events
  `SecureRecordAdded` and `SecureRecordDeleted`.
- `server/src/api/secure.rs`: the service, over the server file's
  `secure_groups`, `secure_records` and `secure_welcomes`
  (`migrations/server/0016_secure_channels.sql`).
- `server/tests/secure.rs`: three people, someone losing access, moderation.
- `web/src/e2ee/engine.ts`: one device for direct messages and secure
  channels; `web/src/components/chat/SecureChannelView.tsx`: the channel.
