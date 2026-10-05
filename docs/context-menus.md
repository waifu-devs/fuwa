# Right-click menus

Every app shows the same menus for the same things. This is the list, for the
web app (`web/src/components/menus/`) and for the desktop app to mirror.

## How they open and close

- Right click, a finger held still on a touch screen (about half a second), or
  Shift+F10 / the Menu key while something inside has focus.
- Rows that drag into order on a hold (channels and categories, for people
  who can arrange them, and servers in the rail) open their menu when the finger lets go without
  having moved, so holding and moving still drags.
- Shift + right click keeps the system's own menu (spelling, saving a link).
  So does right clicking inside a text box that isn't the message box, such as
  a message being edited.
- It opens at the pointer (under the focused element from the keyboard), and
  stays on screen. Escape, a click elsewhere, scrolling, resizing or leaving
  the window closes it. Arrow keys move, Enter picks, Right/Left open and close
  a submenu, typing jumps to an item.
- What it's open for stays lit while it's open.
- One menu at a time: right clicking elsewhere opens a new one there.

## Motion

- Opens from the point it was asked for: scale 0.9 to 1, a few pixels down,
  opacity, on a quick spring (stiffness 700, damping 38, mass 0.6). Closes in
  0.12 s, fading and shrinking a touch.
- Items come in one after another, 12 ms apart (at most ten steps).
- Submenus slide in 8 px from the side they open on.
- The highlight glides between items instead of jumping.
- A held finger sinks the thing held (scale 0.97 over the hold), then a short
  buzz where the device has one.
- With reduced motion, everything only fades.

## What's in them

Items show only when the person may do what they do: the menu gives nothing a
button didn't. Red items remove something or push someone out; those that
can't be undone ask first. "Copy … ID" items show only with Developer Mode.
Sections are drawn with a line between them; an empty one isn't drawn.

### A message (`message`)

| Section | Items |
| --- | --- |
| target | Copy (selected text) · Open link (shows its host) · Copy link · Open picture · Save picture · Copy picture link (pictures from instances the app talks to only): only for what was right clicked |
| react | (reactions, once they exist) |
| primary | Reply in thread / Open thread (where the message's own button shows) · Edit message (yours) · Copy text |
| manage | Keep *name* out (at a shared channel's home, with Kick Members, for someone from another server) |
| developer | Copy message ID |
| danger | Delete message (yours, or with Manage Messages): asks in the message's toolbar |

A join line: Wave · Copy message ID · Delete message. An AutoMod alert: Copy
message ID · Delete alert. A message still sending: Retry (failed) · Copy
text · Dismiss (failed).

### Someone (`member`): their name or avatar anywhere

| Section | Items |
| --- | --- |
| primary | Profile · Message (encrypted DM; not yourself or agents) · Mention (puts `@name` in the message box) |
| social | (friends, once they exist) |
| manage | Edit server profile (yourself) · Change nickname · Roles ▸ (checkboxes for the roles below yours, with Manage Roles; stays open while you pick) |
| moderate | Time out / End time out · Kick · Ban: each opens the same dialog as the profile card, asking for a reason |
| developer | Copy user ID |

### A channel (`channel`)

| Section | Items |
| --- | --- |
| primary | Mark as read · Invite people · Copy link (on the instance's own address, like invite links) |
| notifications | Mute channel ▸ (15 min, 1 h, 3 h, 8 h, 24 h, until turned back on) or Unmute channel · Notifications ▸ (Use the server's, All messages, Only @mentions, Nothing) |
| manage | Edit channel · Permissions · Duplicate channel (not secure or shared channels; with permissions of its own, only with Manage Roles where the copy goes and outranking every role and member they name; the copy is made with its topic, slow mode and permissions in one request, so it is never seen with other permissions; overwrites for people who have left the server are dropped) |
| developer | Copy channel ID |
| danger | Delete channel (asks first) |

### A category (`category`)

Mark as read · Collapse/Expand category · Create channel | Edit category ·
Permissions | Copy category ID | Delete category (asks first; its channels stay).

### A server in the rail (`server`)

| Section | Items |
| --- | --- |
| primary | Mark as read · Invite people |
| notifications | Mute server ▸ / Unmute server · Notification settings |
| manage | Server settings · Edit server profile |
| folder | Put in a new folder, or Take out of folder (your own rail folders) |
| developer | Copy server ID |
| danger | Leave server (not the owner; asks first) |

### A direct message in the list (`dm`)

Mark as read | Copy user ID.

### The message box (`composer`)

Cut · Copy (with a selection) · Paste · Select all | Emoji | a note that Shift +
right click gives spelling fixes. On touch screens the message box keeps the
phone's own menu.

## Adding items

Features add their items without touching the menus: `extendMenu(kind, {
section, at, build })` from `web/src/lib/context-menu.ts`, with the sections
named above. A section a menu doesn't have yet is added before `danger`.
Planned: reactions (a row of recent emoji in `react`), Reply at the start of
`primary`, Pin and Mark unread in `primary`, Report in
`danger`, Add friend in `social`, attachments and polls in the composer's
`insert`.

## Desktop

The desktop app draws the same menus itself (`desktop/src/ui/context_menu.rs`
for opening, keys, submenus, asking first and motion; `menu_items.rs` for
what each holds), with the same sections and checks. Differences: right-click
on a picture in a message adds its items, but links inside a message's text
don't (the text view doesn't say which link was clicked); categories have no
Collapse, as the desktop sidebar doesn't fold them; "Edit server profile" and
"Change nickname" wait for the desktop's server profiles; and the message box
keeps the system's own menu (Cut, Copy, Paste, Select all) with Emoji and
Timestamp added. Shift+F10 and the Menu key open the menu of what the pointer
is over, or of the open channel or conversation.
