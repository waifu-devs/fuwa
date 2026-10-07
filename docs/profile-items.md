# Profile items: effects and decorations from instances and servers

Beyond the effects every app ships with (docs/profile-effects.md), an
instance and each of its servers can offer their own **profile items**:

- **Effects**: animated layers over the profile card, as specs in the
  format of docs/profile-effects.md (data, never code or pictures).
- **Decorations**: a picture around someone's avatar, drawn wherever the
  avatar is: messages, the member list, profile cards.

`ProfileItemService` keeps both (`proto/fuwa/v1/profile_item.proto`).

## Who offers what, and where it shows

| | Offered by | Managed by | Worn on | Shows |
| --- | --- | --- | --- | --- |
| The instance's | node.db `profile_items` | Instance admins (Instance settings > Profile items) | Your profile (`UpdateProfile`: `effect`, `decoration_id`) | Everywhere on the instance |
| A server's | The server file's `profile_items` | Manage Server (Server settings > Profile items) | Your profile in that server (`UpdateMember`: `effect`, `decoration_id`) | In that server, instead of your own |

A member's server profile may also pick a built-in effect. Leaving it empty
shows their own profile's. Moderators with Manage Nicknames can clear
someone's server profile but never put anything on it.

## How apps draw them

Profiles and members name what they wear by id: `Profile.effect` (a
built-in id or one of the instance's items), `User.decoration_id`,
`Member.effect` and `Member.decoration_id`. Apps keep each list and draw from
it:

- The instance's list comes from `ListInstanceProfileItems`, listed again
  whenever `Node.profile_items_at` moves (apps read the Node every minute).
- A server's comes from `ListServerProfileItems` with the server's state,
  and every change sends the whole list as `ProfileItemsUpdated`.

An id an app can't find (deleted, or someone from another instance) draws
nothing. Deleting an item also takes it off everyone wearing it, in the same
write.

`User.decoration_id` travels with the copy of someone's look each server
keeps (its `users` table), so a decoration shows on messages and in member
lists like the avatar does. It never goes to other instances.

## Effects

An item's `effect` is the spec as JSON, checked by the instance before it's
stored (`profile_items::check_effect`): every field in the format's lists and
ranges, at most 16 KB, anything else refused rather than clamped. It's
written back with only the format's fields, under the item's id in lowercase
and its name and description. Apps still put every spec that didn't ship
with them through `sanitizeEffect`, and play it like a built-in one, so the
viewer's switches (Settings > Accessibility > Profile effects), reduced
motion and the slow-device fallback all apply.

Admins and managers add an effect by uploading a `.json` file or pasting the
spec. A theme-style editor can come later.

## Decorations

A decoration is a picture uploaded as `MEDIA_PURPOSE_DECORATION`: for the
instance (no `server_id`, by an instance admin) or for a server (with its
`server_id`, counted in its attachment bytes like emoji). It should be
square and see-through in the middle; apps draw it centred over the avatar
at 1.2 times its size, never catching clicks. A GIF moves (`animated`); with
reduced motion it's held still on its first frame.

A decoration's picture can't be replaced: add another and delete the old
one. Deleting it deletes the picture.

## The instance's switches

- `profile_effects` (`FUWA_PROFILE_EFFECTS`, on) covers every effect,
  built-in or offered.
- `profile_decorations` (`FUWA_PROFILE_DECORATIONS`, on) covers
  decorations. While it's off, profiles come back without one, picking one is
  refused (clearing still works), and apps draw none (`Node.profile_decorations`).

Everyone's picks are kept while a switch is off.

## Privacy

Items carry nothing about anyone. Which item someone wears is part of how
they look, like their avatar: shown to those who can see their profile or
member row, never to other instances, and kept in the account export.
