# Profile items: effects, decorations and nameplates from instances and servers

Beyond the effects every app ships with (docs/profile-effects.md), an
instance and each of its servers can offer their own **profile items**:

- **Effects**: animated layers over the profile card, as specs in the
  format of docs/profile-effects.md: particles of built-in shapes or of
  the effect's own pictures, and a shader (WGSL) drawn under them, alone or
  together.
- **Decorations**: a picture around someone's avatar, drawn wherever the
  avatar is: messages, the member list, profile cards.
- **Nameplates**: a wide picture behind someone's name in the member list
  and the list of direct messages, like a little banner.

`ProfileItemService` keeps all three (`proto/fuwa/v1/profile_item.proto`).

## Who offers what, and where it shows

| | Offered by | Managed by | Worn on | Shows |
| --- | --- | --- | --- | --- |
| The instance's | node.db `profile_items` | Instance admins (Instance settings > Profile items) | Your profile (`UpdateProfile`: `effect`, `decoration_id`, `nameplate_id`) | Everywhere on the instance |
| A server's | The server file's `profile_items` | Manage Server (Server settings > Profile items) | Your profile in that server (`UpdateMember`: `effect`, `decoration_id`, `nameplate_id`) | In that server, instead of your own |

A member's server profile may also pick a built-in effect. Leaving it empty
shows their own profile's. Moderators with Manage Nicknames can clear
someone's server profile but never put anything on it.

## How apps draw them

Profiles and members name what they wear by id: `Profile.effect` (a
built-in id or one of the instance's items), `User.decoration_id`,
`User.nameplate_id`, `Member.effect`, `Member.decoration_id` and
`Member.nameplate_id`. Apps keep each list and draw from
it:

- The instance's list comes from `ListInstanceProfileItems`, listed again
  whenever `Node.profile_items_at` moves (apps read the Node every minute).
- A server's comes from `ListServerProfileItems` with the server's state,
  and every change sends the whole list as `ProfileItemsUpdated`.

An id an app can't find (deleted, or someone from another instance) draws
nothing. Deleting an item also takes it off everyone wearing it, in the same
write.

`User.decoration_id` and `User.nameplate_id` travel with the copy of
someone's look each server keeps (its `users` table), so a decoration shows
on messages and in member lists like the avatar does, and so does a
nameplate. It never goes to other instances.

## Effects

An item's `effect` is the spec as JSON, checked by the instance before it's
stored (`profile_items::check_effect`): every field in the format's lists and
ranges, a shader held to the custom shaders' rules, at most 40 KB, anything
else refused rather than clamped. It's
written back with only the format's fields, under the item's id in lowercase
and its name and description. Apps still put every spec that didn't ship
with them through `sanitizeEffect`, and play it like a built-in one, so the
viewer's switches (Settings > Accessibility > Profile effects), reduced
motion and the slow-device fallback all apply.

Admins and managers add an effect by uploading a `.json` file or pasting the
spec. Beside it they add the effect's pictures and write its shader:

- **Pictures**: up to 8, each numbered by its place, which is what a layer's
  `"image"` names. They're uploads like a decoration's
  (`MEDIA_PURPOSE_DECORATION`, a server's counted in its attachments, one
  file each), sent as `NewProfileItem.picture_urls` and kept in
  `ProfileItem.pictures`. `ProfileItemChange.pictures` replaces the whole
  list: new uploads are checked, pictures left out are deleted, and the
  spec is checked again against the new list.
- **Shader**: WGSL, edited on its own (the editor puts it in the spec's
  `shader.code`, and takes it out of a pasted spec), with the backdrop's
  shader starters to begin from and what happened when this device last
  ran it.

To get one started, the page offers:

- **Start from a built-in**: any of the built-in effects' layers, to change.
- **Copy a prompt for an AI**: the format, its limits and an example, ending
  where the person describes the effect they want, for any AI chat. It
  covers pictures and shaders too. What it
  answers is pasted back and checked like any spec. The web's
  `EFFECT_PROMPT` (`lib/profile-items.ts`) and the desktop's are the same
  words.
- **Read the guide**: this format, in docs/profile-effects.md.

A theme-style editor can come later.

## Decorations

A decoration is a picture uploaded as `MEDIA_PURPOSE_DECORATION`: for the
instance (no `server_id`, by an instance admin) or for a server (with its
`server_id`, counted in its attachment bytes like emoji). It should be
square and see-through in the middle; apps draw it centred over the avatar
at 1.2 times its size, never catching clicks. A GIF moves (`animated`); with
reduced motion it's held still on its first frame.

### Making one

Make it 600 × 600 pixels: the avatar is the circle 500 pixels across in the
middle (50 in from each edge). Keep that circle see-through so the face
shows, and put the art around its rim and in the corners, overlapping the
circle's edge only a little. The background must be really transparent (PNG,
GIF or WebP with alpha). It's shown as small as about 40 pixels across, next
to messages, so bold shapes and clear outlines read best; skip text and fine
detail.

Beside "Add a decoration" the apps offer a **template** (that 600 × 600
picture with the avatar's circle shaded; draw around it and hide the circle
before saving) and a **prompt to copy** for an AI that makes pictures, saying
all of the above, which ends where the person describes their decoration.

A decoration's picture can't be replaced: add another and delete the old
one. Deleting it deletes the picture, as deleting an effect deletes its.

## Nameplates

A nameplate is a picture uploaded like a decoration's
(`MEDIA_PURPOSE_DECORATION`, for the instance or a server), made an item of
kind `PROFILE_ITEM_KIND_NAMEPLATE`. Apps draw it behind someone's row in the
member list, and behind the conversation in the list of direct messages:
cropped to fill the row (about 240 × 44 pixels) and lined up on its right
edge, fading out towards the left so the avatar and name stay readable, a
little see-through until the row is pointed at. A GIF moves; with reduced
motion it's held still on its first frame. Apps that know nameplates ask
for the `nameplates` feature.

### Making one

Make it 900 × 150 pixels, six times as wide as it's tall; PNG, GIF or WebP,
see-through or filling the strip. The avatar and name sit over its left
half, which apps fade out, so put the art mostly on the right and keep the
left calm: soft color, no busy detail. It's shown about 240 × 44 pixels, so
bold shapes and clear colors read best; skip text, faces and fine detail.

Beside "Add a nameplate" the apps offer a template (that 900 × 150 picture,
faint where it fades, with the avatar's circle and the name's box marked)
and a prompt to copy for an AI that makes pictures. Like a decoration's, a
nameplate's picture can't be replaced: add another and delete the old one.

## The instance's switches

- `profile_effects` (`FUWA_PROFILE_EFFECTS`, on) covers every effect,
  built-in or offered.
- `profile_decorations` (`FUWA_PROFILE_DECORATIONS`, on) covers
  decorations and nameplates. While it's off, profiles come back without
  either, picking one is refused (clearing still works), and apps draw none
  (`Node.profile_decorations`).

Everyone's picks are kept while a switch is off.

## Privacy

Items carry nothing about anyone. Which item someone wears is part of how
they look, like their avatar: shown to those who can see their profile or
member row, never to other instances, and kept in the account export.
