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
welcome goes only to signed-in devices of people who can see the channel. A
history record also needs sharing to be on and is accepted only straight after
the sending device's own commit that added someone (see below); turning
sharing on or off needs Manage Channels.
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
- **Threads** work, worked out on each device rather than by the server; see
  [Threads](#threads) for what that changes.

Moderation that doesn't need to read still works: moderators (Manage
Messages) delete anyone's message there (its ciphertext goes, and the audit
log notes it), and time-outs, kicks and bans apply.

## Threads

Replying in a thread works in a secure channel too, the same way to look at
as in any channel ([threads.md](threads.md)), but the server takes no part
in it: it can't read the channel, so it can't tell a reply from any other
line.

- **The thread is inside the encryption.** A reply is an ordinary text whose
  encrypted content names the record of the message it's under
  (`DirectMessageText.thread_sequence`) and whether it was also sent to the
  channel (`in_channel`). To the server it's the same padded, encrypted
  record as any other line. Replies don't nest, and a thread can only start
  under text someone wrote; a reply that names anything else shows as an
  ordinary line.
- **Each device sums threads up itself**, from the lines it opened: how many
  replies, the last one, the latest people to reply, archived (by the
  server's `thread_archive_hours`, as for other threads), what you follow and
  what you haven't read. Following is per device and stays on it: replying
  in a thread, or writing its message, follows it, and following or
  unfollowing by hand sticks. Replies kept to their thread don't count as
  unread in the channel, and only notify you in threads you follow.
- **Nothing is fetched to fill a thread in.** The thread panel and list show
  only what this device already has from reading the channel; the app never
  asks the server for particular older records, so the server can't learn
  which records make up a thread. A thread whose message this device never
  had says so. Searching threads searches what's on the device.
- **Start threads and locks are kept by the apps, not enforced.** The apps
  offer "Reply in thread" on a message with no thread only to people with
  Start threads, and a locked thread's composer only to people with Manage
  messages. A lock is a signed line in the channel (`ThreadChange`) that
  every device counts only if its sender has Manage messages there by the
  device's own view of the channel's permissions; the latest one wins. The
  server can't check either (it can't see which records are replies), so a
  modified app could still reply where the apps wouldn't let it; such a
  reply still shows.
- **Deleting a thread's message** drops its thread from every device: they
  stop showing the replies and never pass them on as shared history. The
  replies' ciphertext stays on the server, unreadable to it. Apps don't
  delete the replies one by one, because a burst of deletes right after the
  message's would tell the server which records were its replies, and how
  many. Anyone can still delete their own replies one at a time.
- **Shared history and backups carry threads.** A thread reply is signed
  like any message, so its thread can't be changed when it's passed on, and
  locks are passed on the same way. A message backup keeps each reply's
  thread and every lock (see [e2ee.md](e2ee.md#message-backup)).
- **Old apps** that don't know threads show replies as ordinary lines.

## History and devices

- **People who join later see only what's sent after they join**, unless the
  channel shares history. That's what MLS gives: a new member gets the group's
  secrets from then on, never the old ones.
- **Sharing earlier messages** is a channel setting (off by default) that
  anyone with Manage Channels turns on or off in the channel's encryption
  panel. The change is a record in the channel's log, so every member sees a
  line saying who changed it, and it goes in the audit log. While it's on, the
  device whose commit added someone posts, right after that commit, one
  history record: up to the last 500 messages it has from since sharing was last turned on (about 56 KB), encrypted
  in the new epoch, so only the members after the commit, the new ones
  included, can open it. The server refuses a history record unless sharing is
  on, the last record in the log is a commit from that same device that added
  devices, and nothing came after it, so each addition gets at most one.
  A device that just joined itself waits a few seconds before passing anything
  on, so a member who has been in the channel longer goes first.
- **Shared messages can't be changed, made up or moved.** Each message (and
  edit) in a secure channel is signed by the device that sent it, over the
  channel, the sender, the time and the content. A device that receives
  shared history keeps an entry only if its signature checks, the signing
  device is registered to the person named as sender right now (anyone can
  sign with a key of their own, so a key that isn't theirs proves nothing),
  that person is in the channel, and the channel's log, read from record
  headers without decrypting anything, has a message from that sender and
  device at that place that wasn't deleted. Each place is taken once, and an
  edit only of a message taken with it. Only messages from before this device
  joined and after sharing was last turned on are taken, so what was said
  while sharing was off stays with those who were there. A sharer can still
  leave messages out, or swap two messages that the same device sent. A
  message whose device has since been removed from its owner's account isn't
  passed on. Shared messages show a small "shared" chip and never chime or
  notify. Messages sent before signing existed aren't shared.
- **A new device starts empty**, as with direct messages: MLS can't decrypt a
  message twice, so each device keeps what it read. With a message backup on
  (see [e2ee.md](e2ee.md#message-backup)), a new device restores what the
  account's other devices read, channels included, with the recovery key.
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
  (`migrations/server/0015_secure_channels.sql`).
- `server/tests/secure.rs`: three people, someone losing access, moderation.
- `web/src/e2ee/engine.ts`: one device for direct messages and secure
  channels; `web/src/components/chat/SecureChannelView.tsx`: the channel.
