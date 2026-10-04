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

- **A block never shows.** The blocked person's requests look sent to them but
  never arrive; their direct messages, in a new conversation or an old one,
  get the same answer as a privacy setting ("they aren't taking direct
  messages from you"), and a call won't ring. Blocking a friend reads to them
  as being unfriended; blocking someone you asked withdraws your request. A
  block stops messages both ways: the blocker has to unblock to write.
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

- 30 new requests an hour per account (in memory, reset on restart), and 100
  waiting at once.
- A request runs out after 30 days; the hourly housekeeping deletes it.
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
sending a message, or joining a conversation's call: never once either person
blocked the other; always in a conversation they already have; otherwise as
the recipient's setting says. Friends may also list and claim each other's
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
