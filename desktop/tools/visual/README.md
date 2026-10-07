# Comparing the desktop app with the web app

The desktop app is meant to look like the web app, pixel for pixel where GPUI
allows. These tools show the same screen in both, against the same seeded
instance, and point at what differs. They run on Linux (Xvfb for the desktop,
Playwright's Chromium for the web) and need no GPU: the desktop draws through
Lavapipe.

## Once

```sh
# The server, with the web app built in.
(cd web && pnpm install && pnpm wasm && pnpm build)
cargo build -p fuwa-server --features web          # target/debug/fuwa, or set FUWA_BIN
# The SDK the seed uses, and Playwright with its Chromium.
(cd sdk && pnpm install && pnpm build)
npm i -g playwright && npx playwright install chromium
# Xvfb, and Lavapipe (mesa-vulkan-drivers) where there's no GPU.
```

## Each time

```sh
cd desktop/tools/visual
./start.sh                      # the instance on :18080, seeded the first time (RESET=1 starts over)
./dcargo build                  # the desktop app from this checkout
DISP=77 ./desk.sh start         # the app on display :77, signed in as alice, on the instance's page
DISP=77 ./desk.sh do click 36 158   # the rail's "Waifu Devs" (#general opens)
DISP=77 ./desk.sh shot general  # -> $FUWA_VISUAL_DIR/out/general-desktop.png
node web-shot.mjs general --light   # the same channel on the web -> out/general-web.png
python3 diff.py general         # out/general-side.png (both side by side), out/general-diff.png (red differs)
```

- `web-shot.mjs NAME [ROUTE] --light [--actions file.json] [--width W --height H]`:
  ROUTE defaults to `/{instance}/{server}/{general}`; `{instance}`, `{server}`,
  `{general}`, `{random}`, `{art}`, `{announcements}`, `{lounge}` and people's
  names (`{alice}`) come from `state.json`. Actions are a JSON list run in
  order: `{"click": "css"}`, `{"text": "Settings"}`, `{"rclick": "css"}`,
  `{"hover": "css"}`, `{"mouse": [x, y]}`, `{"at": [x, y]}` (a click at a
  point), `{"press": "Escape"}`, `{"type": "hi"}`, `{"wait": 500}`.
- `desk.sh do click X Y | move X Y | key ctrl+comma | type TEXT | scroll X Y N`
  works in the window's coordinates, the same as the web screenshot's.
  `AS=bob desk.sh start` (and `AS=bob node web-shot.mjs ...`) signs in as
  someone else.
- `probe.py IMAGE row Y [x0 x1] | col X [y0 y1] | px X Y` prints exact colors
  and where they change, for measuring edges, paddings and colors.
- `dcargo` is cargo for the desktop crate. `FUWA_DESK_TARGET` lets several
  checkouts (git worktrees) share one target dir; its `build` copies the
  checkout's binary aside so each one starts its own.

Several people (or agents) can work at once: each takes their own `DISP`, and
screenshot names that don't clash. Everything is written under
`$FUWA_VISUAL_DIR` (default `/tmp/fuwa-visual`): the instance's data,
`state.json` (ids and tokens), `out/`, logs (`desktop-$DISP.log`, where panics
show; `RUST_BACKTRACE=1 desk.sh start` for traces).

## The seeded instance

Accounts alice (the instance's admin), bob, carol, dave, erin and frank, all
with the password `password123!`. Alice owns "Waifu Devs": #general with two
days of messages (markdown, mentions of people and a role, replies, an edit,
a pin, an open poll and a thread), #announcements, #random, #art and the voice
channel Lounge, in two categories; roles Admin, Moderator and Member, with
colors. "Art Club" and "Gaming" make the rail longer. Leave what's there as
it is (others' screenshots count on it): make your own server or channel when
you need something else.

Theme: Sakura, light, on both (`--light` on the web). Fonts rasterize
differently in Chromium and GPUI, so compare positions, sizes, colors and
structure rather than expecting 0%.
