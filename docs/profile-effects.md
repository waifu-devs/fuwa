# Profile effects

A profile effect is an animated layer over someone's profile card: petals
drifting past, stars blinking on at the edges, a burst of confetti. People
pick one in Settings > Profile; it plays wherever their card opens (the
profile popout from a name or the member list, and the live preview while
they edit).

## How it plays

- **Intro, then idle.** When a card opens, the effect plays a short, dense
  intro (under three seconds), then settles into a sparse loop that runs
  for as long as the card stays open. Pointing at the card plays the intro
  again.
- **Edges first.** Particles start at the card's edges and corners and cross
  it quickly, so the name, status and bio stay readable.
- **On the compositor.** Particles move only by `transform` and `opacity`,
  so the compositor runs them. The page's own thread does no work per frame.
- **Calm when asked.** With reduced motion on (the app setting, or the system
  one by default) the effect shows as a still: a few particles at the edges,
  not moving. Effects pause while their card is off screen.
- **The viewer decides.** Settings > Accessibility > Profile effects turns off
  other people's effects on this device. Your own always shows to you.
- **The instance decides.** Instance settings > Profile effects
  (`profile_effects`, `FUWA_PROFILE_EFFECTS`, on unless set) turns them off
  for everyone. While it's off, profiles come back without an effect and
  `UpdateProfile` refuses to set one (clearing still works). Everyone's pick
  is kept, so it comes back when the setting is on again. Apps hide the
  picker while `Node.profile_effects` is false, which is also what an
  instance from before profile effects reports.
- **Slow devices.** The web app times the intro's frames. If the slowest 5 %
  take longer than 34 ms, the rest of the visit plays lite effects (half the
  particles, no glow), and an anonymous `SlowFrames` error for
  `profile-effect/<id>` goes into the app's report. The 95th percentile also
  goes in as the `profile-effect/intro-frame-p95` timing.

Effects carry nothing about anyone, so streamer mode leaves them alone.

## Where the pick is kept

`Profile.effect` holds the effect's id, such as `sakura`, or nothing.
`UpdateProfileRequest.effect` sets it: lowercase letters, digits and dashes,
up to 32 characters, not starting with a dash. The server checks only the
shape, so a newer app can ship new effects without a server update. An app
that doesn't know an id shows no effect. The pick is in node.db
(`accounts.profile_effect`) and in the account export.

## The format

Every effect is a spec: data, not code or pictures. The built-in specs are
in `web/src/lib/effects/profile.ts` (`BUILTIN_EFFECTS`), and the desktop app
should draw the same ones from the same data. Custom effects will be specs
too, uploaded like theme files. That's why the format holds only shapes,
motions and colors from fixed lists, and numbers held to ranges
(`sanitizeEffect`).

```json
{
  "id": "sakura",
  "name": "Sakura",
  "description": "A gust of petals, then a gentle fall.",
  "layers": [
    {
      "shape": "petal",
      "motion": "burst",
      "phase": "intro",
      "count": 18,
      "from": "top-left",
      "size": [18, 28],
      "duration": [1500, 2300],
      "delay": [0, 260],
      "colors": ["#ffb7c5", "#ff8fab", "#ffc8d6", "primary"],
      "spin": 540,
      "sway": 280,
      "flip": true,
      "opacity": 0.95
    }
  ]
}
```

| Field | Values |
| --- | --- |
| `id` | Lowercase letters, digits and dashes, up to 32 |
| `name`, `description` | Up to 40 and 120 characters |
| `layers` | 1 to 6 |
| `shape` | `petal`, `star`, `sparkle`, `heart`, `snowflake`, `bubble`, `dot`, `confetti`, `streak` (SVG paths in a 24 × 24 box, `SHAPE_PATHS`) |
| `motion` | `fall`, `rise` (across the card, swaying), `twinkle` (a glint in place, then a rest), `drift` (wanders and glows), `burst` (flies out and falls away), `shoot` (a streak, then a rest), `pop` (swells and pops) |
| `phase` | `intro` plays once when the card opens; `idle` loops after it |
| `count` | 1 to 24, for a 304 × 420 card (counts scale with the card's area) |
| `from` | `top`, `bottom`, `edges`, `corners`, `anywhere`, `top-left`, `top-right`, `center` |
| `size` | Pixels, 2 to 96 |
| `duration` | One pass, 300 to 20000 ms |
| `delay` | Intro start times, 0 to 3000 ms |
| `colors` | Theme tokens (`primary`, `ring`, `accent`, `foreground`, `card`), `profile` (the card's own color) or `#rrggbb`; up to 8 |
| `spin` | Degrees turned over a pass, ±1440 |
| `sway` | Pixels: sideways sway, wander radius or flight distance, 0 to 400 |
| `flip` | Tumbles in 3D, like paper |
| `glow` | A soft glow in the particle's color |
| `opacity` | Peak opacity, 0.05 to 1 |

A card plays at most 60 particles at once. Each particle's randomness
(where it starts, its size, color and timing) is seeded from the person's
id, so someone's effect looks the same every time they're seen, and a
little different from someone else's.

`planEffect` turns a spec into particles, each a list of keyframes
(`transform`, `opacity`, `offset`) with a duration, delay, iteration count
and easing, plus the still frame shown under reduced motion. The desktop app
ports `planEffect` (it's pure arithmetic over the spec and a seeded random
number generator, mulberry32 over an FNV-1a hash) and plays the plans with
its own animation system, so both apps place every particle the same way.

## Built-in effects

| id | What it does |
| --- | --- |
| `sakura` | A gust of petals from the top left, then a gentle fall |
| `starfall` | Stars blink on at the edges with a shooting star, then the odd glint and streak |
| `sparkles` | Sparkles pop at the edges and keep glinting |
| `hearts` | Hearts burst up from below, then float up |
| `snow` | A flurry, then soft snow |
| `bubbles` | Bubbles pop in at the edges and float away |
| `fireflies` | Little lights that wander and glow |
| `confetti` | A burst of confetti from the top, then a few pieces drifting down |
