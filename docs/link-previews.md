# Link previews

When someone links to a page in a server, a card shows up under the message
with the page's site, title, description and picture, as in other chat apps.
A link straight to a picture shows the picture.

## What people see

- Up to 3 cards per message, one per link, in the order the links come.
  Links in code, behind a spoiler (`||...||`) or wrapped in `<...>` get no
  card: `<https://example.com>` is how to send a link without one.
- The card comes a moment after the message: the instance reads the page
  first, then updates the message (`MessageUpdated`). It isn't an edit, so
  the message isn't marked edited.
- Editing the text makes the cards again for the links it has now.
- The author can take a message's cards off (the × on a card, or
  `MessageService.HideLinkPreviews`). They stay off, edits included.
- A card's words are shown as plain text, never Markdown, since they come
  from another site.

## Privacy

Apps never load the linked page or its picture: the site would learn each
reader's IP address. The instance reads the page once, as itself (a
`fuwa/<version>; link preview` user agent, nothing about who sent the link),
and the picture goes through its picture proxy (`/media/outside/`, see
`server/src/outside.rs`), like every other picture from elsewhere. So the
linked site sees the instance's address, once, and nobody else's.

Pages are read only at public addresses (never loopback, private networks or
cloud metadata), following at most 5 redirects, within 8 seconds, and only
their head (at most 512 KB). What a page said is kept in memory for 30
minutes, so a link pasted in many places is read once. Nothing is written to
disk but the cards themselves, in the message.

## Where previews aren't made

- Direct messages and secure channels: the instance can't read them, and
  apps never fetch the page themselves (docs/e2ee.md,
  docs/secure-channels.md).
- Messages from people without Embed Links in the channel.
- With the instance's `link_previews` setting off (Instance settings → Link
  previews, `FUWA_LINK_PREVIEWS`, on unless set). Cards already made stay.

Webhooks' and agents' messages get cards like anyone's, after the embeds they
sent; a message holds at most 10 embeds in all.

## How it works

- `server/src/previews.rs`: finding the links in a message's text
  (`links`), reading a page's OpenGraph and Twitter card tags, else its
  `<title>` and description (`read_page`), and fetching with the cache
  (`page`).
- `server/src/api/messages.rs`'s `preview_links` runs after a message is
  sent or its text edited (`send_with`, `UpdateMessage`, webhooks, guests'
  messages in a shared channel's home): it reads the pages in the background,
  then in one write replaces the message's preview embeds (`Embed.preview`)
  with the new ones and sends `MessageUpdated`. A message edited again
  meanwhile is left to the newer edit's previews. The message's extras keep
  `previews_hidden` once its author hides them.
- Apps send embeds of their own (`SendMessageRequest.embeds`) never as
  previews: the instance clears `preview` and `site_name` on them.
- In a channel shared with another instance, cards travel with the message,
  their pictures through the reader's own instance (docs/federation.md).
- `FUWA_LINK_PREVIEWS_ALLOW_PRIVATE=1` (environment only) lets the instance
  read pages on private addresses: for tests and private deployments.
- Web: `components/chat/Embeds.tsx` (`LinkPreview`). Desktop:
  `ui/embeds.rs`.
