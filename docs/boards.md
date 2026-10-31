# Boards

A board is a channel type for tracking work: columns ("To do", "Doing",
"Done") holding cards people drag between them. Each card has a title, a
body, people assigned, labels and a due date, and a conversation of its own
under it. It's for a server that plans an event, triages bug reports, runs
a mod queue or tracks fan art commissions, without leaving the server for
Trello. Instances that have them list `boards` in `Node.versions.features`.

## What people see

- In the sidebar a board is a channel with a kanban icon. Opening it shows
  the board where the message list would be: columns side by side,
  scrolling sideways, each with its cards scrolling up and down, and an
  "Add a card" at the bottom of each column (a title, then Enter).
- A card shows its labels as small colored chips, its title, a due date
  (red once it's past, unless the card is in a done column), how many
  comments and files it has, and the people assigned.
- Dragging a card moves it, within its column or to another; everyone sees
  it glide to its new place. Alt+arrows move a focused card too.
- Opening a card shows it beside the board (over it on a narrow screen):
  its title, column, people, labels and due date, its body, and the
  conversation under it.
- A filter bar narrows the board to my cards, a label, or some words in a
  card's title or body. Filters stay on the device.
- Cards can be archived and restored; "Archived cards" pages through them.

## Model

A board is `CHANNEL_TYPE_BOARD = 7`. **A card is a message**: its body is a
message in the board's channel whose id is the card's id, and `board_cards`
(migration `0042_boards.sql`) holds what makes it a card. So the body is
Markdown with attachments, mentions, emoji (the server's and other
servers'), reactions and AutoMod, all as in any message, through the same
send and edit paths. Every message in a board is a card's body or a comment
on one.

- `board_columns`: name, color, position, `done` (cards there count as
  finished), `card_limit` (a soft limit, shown, never enforced).
- `board_labels`: name, color, position; shared by the board's cards.
- `board_cards`: column, `rank`, title, label ids and assignee ids (space
  separated), due date, author, created, updated and archived times.
- `board_assignees`: one row per card and person, to find someone's cards.

`rank` is a fractional index (base-62 digits `0-9A-Za-z`, never ending in
0): a card dropped between two others gets a key between theirs, so a move
writes one row and two people moving different cards never clash. When two
cards share a rank (moved to the same place at once), the next move there
spreads the column's ranks out. Cards sort by rank, then id. `between` is
the same in `api/boards.rs` and `web/src/lib/board.ts`, so an app's
optimistic move lands where the server will put it.

Comments are a thread: a comment is a message with `thread_id` set to the
card's id, so `threads`, `ThreadSummary`, follows and unreads come along.
Starting a card's thread doesn't need Start threads. Creating a card follows
its thread for its author, and assigning someone follows it for them.

Making a board makes three columns: "To do", "Doing" and "Done" (done).

## API

`fuwa.v1.BoardService` (`proto/fuwa/v1/board.proto`, `api/boards.rs`),
every request with `server_id` as field 1 and `channel_id` as field 2,
routed to the shard:

- `GetBoard`: columns, labels, every card not archived, their bodies (with
  reactions and thread summaries) and the people they name.
- `ListArchivedCards`: archived cards, latest first, paged by `after_id`.
- `CreateCard` (column, title, body content, attachments and emojis like
  `SendMessage`, labels, assignees, due date, and `before_id` to place it;
  at the bottom otherwise). It's `SendMessage` underneath (`send_with` with
  a `NewCard`), so slow mode, AutoMod (title and body), mentions and files
  apply.
- `UpdateCard` (a `CardChange`: title, due date, labels, assignees,
  archived, and the body's content, emojis and whole attachment list; each
  only when set). A new body runs AutoMod again, and files taken off are
  deleted.
- `MoveCard` (card, column, `before_id`; "" for the bottom): the server
  works out the rank from the cards there now.
- `SetBoard`: the whole list of columns and labels, new ones without ids. A
  column can't be removed while it holds cards that aren't archived; the
  archived ones move to the first column. Labels removed come off cards.
  Audited as a channel update.
- Deleting a card is deleting its body (`MessageService.DeleteMessage`),
  which takes its comments, reactions and files (`threads::after_delete`
  calls `boards::forget_card`).

Events, all scoped to the channel (`events::channel_of`), so anyone who
can't see the board never gets them: `BoardUpdated` (columns and labels),
`CardUpdated` (the whole card: made, moved, edited, archived),
`CardDeleted`. Bodies and comments are `MessageCreated`/`Updated`/`Deleted`
and `ThreadUpdated`, as anywhere. On a `LiveService` connection card events
always come through; a board's messages are held back when it's out of
focus like any channel's, and the app reads the board again with `GetBoard`
whenever it's shown.

## Permissions

- **View Channels**: see the board and its cards.
- **Manage Cards** (`PERMISSION_MANAGE_CARDS`, new): add (with Send
  Messages), move, edit and archive cards. Migration 0042 gives it wherever
  Send Messages was given or denied (roles and channel overwrites), so
  servers behave as before until they change it; a board where only some
  people move cards denies it to @everyone in the channel and allows it to
  a role. It's a channel permission and one of the talking ones a member
  who hasn't agreed to the rules doesn't have.
- **Send Messages**: comment on cards.
- **Manage Messages**: delete anyone's card (its author can delete their
  own), lock a card's thread.
- **Manage Channels**: columns and labels.
- Assigning someone needs them to be able to see the board.
- Polls, buttons, interactions and "also send to channel" aren't on boards;
  webhooks, invites and live tiles don't point at them; they can't be
  shared with other servers or be secure.

## Everything else

- **AutoMod** reviews a card's title and body in the write that creates or
  edits it, as it does a message; a blocked card is refused with
  "AutoMod: ". The Smart filter (a provider) sees a new card's body, not
  edits made through `UpdateCard`.
- **Search** finds card bodies and comments (they're messages); titles
  aren't indexed yet.
- **Agents and MCP**: board events reach agents through their `View` like
  any channel's, and `mcp/manage.rs` has `get_board`,
  `list_archived_cards`, `create_card`, `update_card`, `move_card` and
  `set_board`. @fuwafuwa builds a roadmap board's approved cards in
  boat.dev sandboxes (`agents/fuwafuwa/README.md`).
- **Usage and exports**: bodies and comments are messages, counted and
  exported as such. Deleting an account takes its assignments
  (`forget_assignee`); its cards stay, like its messages.

## Apps

Web: `lib/board.ts` is pure (ranks, applying events, where a drop lands,
keyboard steps, filters, due dates); `fuwa/board.ts` the store per board
and the calls (a move or edit shows at once and goes back if refused);
`hooks/use-card-drag.ts` drags cards on the DOM like `use-arrange.ts` (a
copy moved by `transform`, a drop line, one render on drop; a finger holds
first); `components/board/` holds `BoardView` (the page, columns, filters,
archived list), `BoardCard`, `CardPanel` (fields, the body shown with
`MessageBody` and `Attachments` and edited with its files, comments as
`MessageList` and `Composer` on the card's thread) and
`BoardSettingsDialog` (columns and labels).

Desktop: `core/board.rs` mirrors `lib/board.ts` and `fuwa/board.ts` (the
same `between`, events applied to the boards open in `InstanceState`, and
the calls, a move or edit shown at once and put back if refused);
`ui/board.rs` draws the board where the message list would be, each column
a `widgets::inner_scroll`, cards dragged with GPUI's drag and drop (the
upper or lower half of a card says above or below it) and moved, archived
or deleted from their right-click menu. The card panel's comments are the
thread panel's rows and reply box on the card's thread (`threads.rs`), and
its description is edited with the composer's files (`attachments.rs`),
spoiler marks included. Column, people, label and filter pickers are
context menus (`MenuOf::Board`). Board settings (columns and labels) are
web only for now.

## Later

Board settings on desktop; a body editor with the composer's @ and emoji pickers; search over
titles; the Smart filter on edits; assignment notifications; right-click
menus on cards (`docs/context-menus.md`); archiving after some days in a
done column; webhooks that make cards; templates, checklists, a "due soon"
live tile, and list or calendar views of the same cards.
