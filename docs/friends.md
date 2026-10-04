# Friends

People on an instance can be friends: ask by username or from someone's
profile (in chat, a member list, anywhere a profile card opens), take or turn
down a request, cancel one, unfriend, and block. Friends see each other online,
can open a direct message without a server in common, and each person decides
who may ask them and who may start a conversation with them.

## What's private

A friend list is its owner's alone. No other member, no server, no agent and
no other instance ever sees whose friends someone is, who they asked, or who
they blocked. Nothing about friends goes in a server's event log or audit log;
the only people told about a change are the two it's between, and a block is
told to nobody.

- **A block never shows.** Nothing the blocked person can do gets an answer
  a block would explain. Their requests look sent but never arrive. Their
  direct messages are taken as usual and kept from the blocker, who reads
  them as deleted before read (the ciphertext is left out for them only); a
  new conversation opens for them without the blocker hearing of it, though
  the blocker's settings still refuse it as they would anyone. Their calls
  never ring: the blocker isn't told of the call and can't join it. Blocking
  a friend reads to them as being unfriended; blocking someone you asked
  withdraws your request. The blocker has to unblock to write or call, and
  a blocker in a call with them is taken out of it.
- Hiding is decided when the blocker reads, not when the message is sent:
  while the block lasts nothing from that person shows on any of their
  devices, and the blocker's list keeps that conversation where it was when
  they blocked (one the blocked person opened since isn't in it at all, nor
  are its welcomes). After an unblock, a device that reads the conversation
  again (a new one, or a full re-sync) gets what was sent during the block.
- Single-use key packages: anyone claiming another person's devices gets a
  few an hour per device, partners included, then the device's last-resort
  one; so a partner who blocked you answers exactly as one who didn't.
- Profiles only answer for people you'd see anyway: friends, requests either
  way, people in a server with you and people you've talked with. Anyone
  else is "not found".
- **Declining is silent.** The sender's request stays waiting on their side
  until it runs out, as if it were never answered. If the person who declined
  later asks them, it's a friendship at once (they did ask).
- **Online** means having a fuwa app open (a `WatchFriends` stream), shown
  only to friends, and only for people who don't hide it.
- **Mutual friends** show on a profile only when the viewer, the person
  viewed and each shared friend all allow it.
- Addresses are never kept, logged or limited by: limits are per account.

## Settings (`FriendSettings`)

Every default is as open as fuwa was before friends.

| Setting | Choices | Default |
| --- | --- | --- |
| Who can send you friend requests | everyone on the instance, people in a server with you, nobody | everyone |
| Who can start a conversation with you | friends and people in a server with you, friends only, nobody new | friends and server members |
| Show when I'm online | on, off | on |
| Show mutual friends | on, off | on |

Conversations someone already has keep working whatever the conversation
setting says; blocking is what stops one. The settings follow the account
(node.db), so they hold on every device and app.

## Limits

- 30 new requests an hour per account (in memory, reset on restart; taken
  before sending and given back if no request came of it), and 100 waiting
  at once (each request writes the sender's `friend_senders` row, so two at
  once clash and the second counts again).
- A request runs out after 30 days; the hourly housekeeping deletes them,
  500 at a time.
- Agents have no friends and can't be asked.

## How it's kept

node.db (`migrations/node/0013_friends.sql`), on a split instance's
directory, which answers every `FriendService` call:

- `friend_links (account_id, other_id, state, created_at, expires_at)`: each
  person's own row about the other (`FriendState`: friend, outgoing, incoming,
  blocked), so a block or a declined request stays on one side. Changes that
  touch both people write both rows in one transaction, so two at once (each
  asking the other) clash and one runs again on what the other left.
- `friend_settings`: one row per person who changed a setting.

`other_id` points at nothing on purpose: someone on another instance would be
`<id>@<host>` (docs/federation.md), so friends across instances fit the table
later without changing it. Federation itself isn't part of friends yet.

Deleting an account deletes every row about it either way, and tells those
who had it in their list. The data export includes your own list and
settings.

## Direct messages

`DirectMessageService` asks `Api::may_message` before opening a conversation,
sending a message, or joining a conversation's call: never once the sender
blocked the other; always in a conversation they already have; otherwise as
the recipient's setting says. When the recipient blocked the sender it says
so to the server only (`Reach::Hidden`): `ListRecords` and `Watch` leave the
blocked person's messages' ciphertext out for the blocker (their commits
still arrive, so the group keeps working after an unblock), and call updates
skip the blocker. Friends may also list and claim each other's
devices' key packages, as people in a server together may. None of this
touches encryption: what the server checks is still only who is talking to
whom (see docs/e2ee.md).

## Apps

The web app keeps the list in `fuwa/store.ts` (`friends`), follows it in
`fuwa/friends.ts` alongside the instance's event stream, and draws it in
`pages/FriendsPage.tsx` (`/<instance>/friends`, tabs for online, all, pending
and blocked, only the lines in view drawn), the Friends link with its request
badge in the instance's sidebar, `components/friends/FriendActions.tsx` on
profile cards, and Settings > Friends and privacy. The pure parts (events,
tabs, counts) are `lib/friends.ts`, tested in `lib/friends.test.ts`.

The desktop app needs the same: a Friends screen under the instance's home
with the four tabs and an add box, the badge, the buttons on its profile
card, the settings page, following `WatchFriends` in its sync task, and
hiding conversations with people you blocked.
