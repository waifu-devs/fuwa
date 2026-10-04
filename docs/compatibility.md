# Compatibility dates

fuwa's apps and instances are updated on their own schedules: a desktop app
can be weeks behind the instance it talks to, a self-hosted instance can be
behind the web app on fuwa.chat, and nobody is ever forced to update. So
every feature an app has to know about to use carries a **compatibility
date**, the day it arrived, and both sides say which features they have.

## The one list

[`proto/fuwa/v1/features.json`](../proto/fuwa/v1/features.json) is the only
place features are declared:

```json
{ "id": "shared-channels", "date": "2026-10-04", "title": "Channels shared between servers" }
```

- **The server** builds it in (`server/src/compat.rs`) and sends it to every
  app in `GetNode`, as `Node.versions`: its own compatibility date (its
  newest feature's), `min_client_date`, and the features.
- **The web app** builds in a copy, `web/src/gen/features.json`, which
  `pnpm generate` makes (a test fails if the copy is out of date);
  `web/src/lib/compat.ts` reads it.
- **The desktop app** builds it in too (`desktop/src/core/compat.rs`).

An app's own compatibility date is its newest feature's.

## What an older app does

- When an instance lists a feature the app doesn't know, the app says
  **"Update fuwa to use …"** (the feature's title, from the instance): a
  small note on the web, the update card and the Updates page on desktop.
  Everything else keeps working.
- When the app's date is older than the instance's `min_client_date`, it
  says **"Update fuwa so everything works"**. Nothing is refused.
- When the instance doesn't have a feature the app knows (an older
  instance), the app hides that feature's screens: `instanceHas` /
  `instance_has`. An instance from before compatibility dates counts as
  having every feature dated 2026-10-04 or earlier.

Nothing here ever updates anything by itself; it only says what an update
would bring.

## Adding a feature

1. Add it at the end of `features.json`, dated the day it merges (the list
   stays in date order, ids are unique; `cargo test` checks both). Never
   change or remove a date.
2. Run `pnpm generate` in `web/` so the web app's copy matches.
3. In the apps, show the feature's screens only where the instance has it
   (`instanceHas(node?.versions, "<id>")` on the web,
   `compat::instance_has(...)` on desktop), and give anything an older app
   could meet (a new kind of channel or message) a plain "Update fuwa to use
   this" in place of a broken screen.
4. Raise `min_client_date` only when apps older than it would get something
   wrong rather than just miss something new, and say why in the PR.
