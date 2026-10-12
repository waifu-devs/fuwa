# Mod View

Mod View is where a server's moderators see what needs their attention and
look into one person. It covers:

- **Cases**: every message AutoMod catches, and every message or member that
  members report, waiting for a moderator to resolve or dismiss it.
- **A person's page**: their membership and account age, any ban, the cases
  about them, what moderators (and AutoMod) did to them, their messages, and
  the moderators' private notes about them.

It's `ModerationService` in `proto/fuwa/v1/moderation.proto`, served by the
shard that keeps the server (`server/src/api/moderation.rs`). Everything
lives in the server's own file.

## Who can use it

The **Use Mod View** permission (`PERMISSION_MODERATE`) is server-wide only.
New servers give it to their Admin role. When a server first runs this
version, every role that could time out, kick or ban members gets it too.

Mod View shows things. It never acts: closing a case doesn't time anyone out,
and timing someone out from a person's page still needs Time Out Members.
Rank works as everywhere else.

A moderator sees only what's in channels they can see. A case about a
message in a channel hidden from them doesn't exist for them: it's left out
of lists, counts and events, and fetching it answers NotFound. A person's
messages are listed only from the channels the moderator can see, and their
history leaves out the names of channels the moderator can't see.

## Cases

| Kind | Opened by | About |
| --- | --- | --- |
| `AUTO_MOD` | AutoMod, for each rule that catches a message (blocked, flagged or timed out) | the author |
| `MESSAGE_REPORT` | a member's `ReportMessage` | the message's author |
| `MEMBER_REPORT` | a member's `ReportMember` | that person |

A case keeps a copy of what the message said when it was caught or first
reported, so moderators can still read it after it's edited or deleted (a
ban's purge, say). A blocked message was never sent, so its case has no
`message_id`.

Reports of the same message, or of the same person, join the case that's
still open (`mod_cases.open_key`): one row per reporter, so reporting again
changes your reason and note rather than counting twice. Once that case is
closed, the next report opens a new one. Every report updates the case's row,
so two reports at the same moment clash and one runs again, and the count
stays right.

A case is `OPEN` until a moderator marks it `RESOLVED` (dealt with) or
`DISMISSED` (nothing to do), with an optional reason. Either can be opened
again. Each change is in the audit log (`MOD_CASE_UPDATE`, with the person
as its target).

### Who reports

Any member who can see a message can report it, except its author. Anyone
can report a person who is or was in the server, except themselves. Reports
work while you're timed out: being timed out is when someone may most need
to report what was said to them.

The person reported is never told. Moderators see who reported and why.
Reporting is never capped.

Secure channels and direct messages can't be reported: the server can't read
them, and a report would have to carry the plaintext out of the encryption.
Messages in a channel shared from another server are kept by that server, so
they can't be reported from here yet.

## Notes

Moderators can write plain-text notes about a person, up to 1,000 characters
each. Only members with Use Mod View see them. You can delete your own notes,
and anyone with Manage Server can delete any note. Adding and deleting notes
is in the audit log (`MOD_NOTE_ADD`, `MOD_NOTE_DELETE`), without the text.

## Events

`ModCaseUpdated` (a case opened, reported again, or closed), `ModNoteAdded`
and `ModNoteDeleted` are stored in the server's log. Only members with Use Mod
View get them, and a case about a message only if they can see its channel
(`events::shown_to`). A `ModCaseUpdated` carries the case: its counts and
reasons, never who reported or their notes. `GetModCase` has those.

## When an account is deleted

The cases and notes about that person go. So do their reports, and a report
case left with no reports goes with them. Notes they wrote stay, under the
placeholder name their account becomes.

## Apps

The web app and the desktop app show Mod View from a server's menu to members
who have the permission:

- a queue of cases (open first) with each case's message, reports and
  actions;
- a person's page, reached from a case, a profile card or the member list;
- "Report message" in a message's menu and "Report" on profile cards, for
  everyone.
