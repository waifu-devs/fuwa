# Themes

How fuwa looks: built-in themes, themes people make, and the backdrop (a
background picture and an effect) behind the app. Every fuwa app (web and
desktop) reads the same themes and the same theme files, so a theme looks
the same everywhere. The web app's code is the reference: `web/src/lib/themes.ts`,
`theme-file.ts`, `backdrop.ts` and `effects/`.

## Tokens

A theme is a full set of values for the shadcn/ui tokens, plus a corner
radius:

| Token | What it colors |
| --- | --- |
| `background`, `foreground` | the page and its text |
| `card`, `card-foreground` | cards, the composer |
| `popover`, `popover-foreground` | menus and popovers |
| `primary`, `primary-foreground` | buttons, links, the selection |
| `secondary`, `secondary-foreground` | quieter buttons |
| `muted`, `muted-foreground` | quiet backgrounds, secondary text |
| `accent`, `accent-foreground` | hover and the open channel |
| `destructive` | danger |
| `border`, `input`, `ring` | lines, fields, focus |

Colors are `#rrggbb`. The radius is in rem, from 0 to 1.5.

Seven of them are the **seeds** (`background`, `foreground`, `card`, `primary`,
`primary-foreground`, `muted-foreground`, `border`); the others are made from
them by `deriveTokens` (in sRGB, `mix(a, b, t)` is `a * t + b * (1 - t)` per
channel, rounded):

```
card-foreground = popover-foreground = secondary-foreground = accent-foreground = foreground
popover = card
secondary = mix(primary, card, 0.12)
muted = mix(border, background, 0.45)
accent = mix(primary, background, 0.14)
destructive = #e5484d
input = border
ring = primary
```

The app's surfaces come from the tokens too: the server rail is
`mix(primary, muted, 0.09)` (dark themes: `mix(background, black, 0.78)`), the
channel sidebar `mix(card, background, 0.55)`, the chat `background`. A theme
is dark when its background's luminance (`0.2126 r + 0.7152 g + 0.0722 b`,
0 to 255) is under 128.

## Built-in themes

The same five as waifu.dev (`packages/domain/src/themes.ts` in
waifu-devs/site), seeds and radius:

| id | background | foreground | card | primary | primary-foreground | muted-foreground | border | radius |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `sakura` | #fff5f8 | #3b2330 | #ffffff | #f06292 | #ffffff | #8a6577 | #f8d3e0 | 1 |
| `yoru` | #14111f | #ece6ff | #1f1a2e | #b388ff | #14111f | #9a90b8 | #342b4d | 0.75 |
| `matcha` | #f4f6ec | #243021 | #fffef7 | #5a8a3c | #ffffff | #66735f | #d9e2c8 | 0.5 |
| `sora` | #f0f7ff | #1a2b44 | #ffffff | #3b8beb | #ffffff | #5f7391 | #cfe2f7 | 1.25 |
| `tsundere` | #1a0f12 | #ffe9ec | #2a171c | #ff4d6d | #1a0f12 | #c28c96 | #4a2630 | 0.25 |

Sakura is the default on a light system, Yoru on a dark one. People can
follow the system with a light and a dark pick.

## Custom themes

Made in Settings, Themes: from any theme, with its colors, corners and
(optionally) its own backdrop. They're app settings: kept on the device
(the web app's `fuwa:prefs:v1`, `customThemes`; the desktop app's config
file), at most 50, with ids `custom-` and 4 to 32 lowercase letters and
digits.

## Backdrops

What's behind the app. The app has one (Settings, Background) for every
theme, and a custom theme may bring its own, used while it's on screen.

| Field | Values | Default |
| --- | --- | --- |
| `image` | a picture on a fuwa instance (`https://<instance>/media/<id>`), or `""` | `""` |
| `fit` | `cover`, `contain`, `tile` | `cover` |
| `dim` | 0 to 90: how much the theme's background covers the picture, in % | 35 |
| `blur` | 0 to 24, in pixels | 0 |
| `effect` | `none`, `aurora`, `petals`, `stars`, `waves`, `custom`, `grain`, `paper`, `dots`, `grid` | `none` |
| `intensity` | 0 to 100, % | 70 |
| `speed` | 0 to 200, % of normal (0 is still) | 100 |
| `panels` | 20 to 100: how solid the chat is over it, % (the rail and sidebars are 20 points more) | 35 |
| `shader` | a custom shader (below), drawn when `effect` is `custom`, or `null` | `null` |

Layers, back to front: the page's `background`, the picture (with `fit` and
`blur`), `background` again at `dim`, the effect, then the app with its
surfaces at `panels`.

**Pictures** are uploaded to an instance (`MediaService.CreateUpload` with
`MEDIA_PURPOSE_BACKGROUND`, then `KeepBackground`), up to 24 per account,
listed with `ListBackgrounds` and removed with `DeleteBackground`. Apps show
a picture only from an instance they're signed in to (or chose to open), never
from anywhere else. The web app scales pictures to at most 2560 pixels on the
long side and saves them as WebP (GIFs go as they are).

**Effects** `aurora`, `petals`, `stars` and `waves` are fragment shaders
(`web/src/lib/effects/shaders.ts`, WGSL; the web app runs them with
[vgpu](https://vgpu.sh) on WebGPU, bundled in the app). They take the
theme's primary, the primary turned 48° in hue, and the background, and
draw premultiplied alpha. They run at most 30 frames a second, aurora at
half resolution and waves at three quarters, and stop while the window is
hidden or settings cover the app; with reduced motion they draw one frame.
Where WebGPU isn't there, a CSS version stands in (`styles/app.css`,
`.fx-*`). `grain`, `paper`, `dots` and `grid` are still textures (CSS, with
SVG noise inline, never a file from elsewhere). The desktop app draws its
own version of the same effects with GPUI's shapes (`desktop/src/ui/effects.rs`:
soft shadows for light, paths for petals and waves, small quads for stars),
at the same 30 frames a second, still while its window is behind others.

## Custom shaders

People can write their own effect (Settings, Background, Custom; or a
custom theme's own backdrop), and a theme file carries it. A shader is:

| Field | Values | Default |
| --- | --- | --- |
| `name` | up to 32 characters, shown on the effect card | `My shader` |
| `code` | WGSL, at most 16 KB (UTF-8), defining `fn shade(uv: vec2f) -> vec4f` | |
| `fallback` | `none`, `aurora`, `petals`, `stars`, `waves`: what shows where the shader can't run | `aurora` |

`shade` is called for every pixel and returns a color and how much of it
covers what's behind (straight alpha, 0 to 1). The app puts it between its
own prelude and entry point (`web/src/lib/effects/custom.ts`, `fullSource`),
which apply the strength (`intensity`) and premultiply. Everything it can
read:

| Input | What it is |
| --- | --- |
| `uv` | the pixel, 0 to 1 across and down; (0, 0) is the top left |
| `fuwa.time` | seconds (`f32`), scaled by `speed`; still with reduced motion |
| `fuwa.res` | the size drawn, in pixels (`vec2f`) |
| `fuwa.pointer` | the pointer in `uv`'s units (`vec2f`), eased (about 1/7 s); starts at (0.5, 0.5), outside 0..1 when it's off the canvas |
| `fuwa.primary`, `fuwa.accent`, `fuwa.background`, `fuwa.foreground` | the theme's primary, the primary turned 48° in hue, its background and its text (`vec4f`, alpha 1) |
| `fuwa.intensity` | the strength, 0 to 1 (already applied) |
| `hash21(p)`, `noise(p)`, `fbm(p)` | a random number, value noise and four octaves of it, 0 to 1 |

The uniform is `struct Fuwa { primary, accent, background, foreground: vec4f, res, pointer: vec2f, time, intensity: f32 }`
at group 0, binding 0. Apps must give exactly these inputs, so a shader draws the same everywhere.

What keeps a shader to these inputs, in every app:

- **Checked as text first** (`shaderProblem`): no link (`scheme://`)
  anywhere, comments included; then, with comments left out, no `@` at all
  (no bindings, no entry points of its own, so no textures, buffers or
  samplers), no `enable`, `requires` or `diagnostic`, and a `fn shade(`.
  Comments are read the way the compiler reads them (`stripComments`): left to
  right, `//` to the next WGSL line break (`\n`, `\v`, `\f`, `\r`, U+0085,
  U+2028, U+2029), and `/* */` nesting, so nothing the compiler sees as code is
  skipped. Control characters other than tabs
  and line ends are dropped on read. WGSL has no way to load anything, so
  this is all a shader can reach.
- **Compiled before it draws.** One that doesn't compile never draws; the
  editor shows the compiler's errors at the shader's own lines and keeps the
  last version that worked.
- **Timed.** Its first frames are timed at three quarters, half and a third
  of the resolution until one takes at most 20 ms on the GPU; if none does,
  it's too slow for the device. While it runs it's timed every two seconds,
  and three slow samples in a row (over 40 ms) drop it a step or stop it.
- **Remembered when it hangs.** Before its first frames the app notes it as
  being tried (the web app's `fuwa:shaders-trying`) and clears the note once
  they come back. A frame that hasn't come back in 2.5 s, a GPU lost while it
  draws, or a note left from last time (the tab died) means it stopped the
  GPU: it isn't run again until it's changed or someone asks to try again.

In every one of these cases, and where there's no WebGPU, the backdrop shows
the shader's `fallback` instead, and Settings says why. Custom shaders run at
most 30 frames a second like the built-in ones, and stop the same way.

## Theme files

A theme travels as a JSON file named `<name>.fuwa-theme.json`:

```json
{
  "format": "fuwa-theme",
  "version": 1,
  "name": "Midnight Sakura",
  "description": "Blossoms after dark.",
  "colors": { "background": "#16101a", "foreground": "#ffe9f1", "...": "every token" },
  "radius": 1,
  "backdrop": {
    "image": "data:image/webp;base64,...",
    "fit": "cover",
    "dim": 45,
    "blur": 4,
    "effect": "petals",
    "intensity": 60,
    "speed": 100,
    "panels": 80,
    "shader": null
  }
}
```

- `colors` has every token; a file with only the seven seeds is read too,
  with the rest made from them. So is a plain waifu.dev theme
  (`{ "tokens": {...}, "radius": 1 }` or `{ "variant": {...} }`).
- `backdrop` is optional (null: the theme uses the app's backdrop).
- `image` is the picture itself, as a `data:` URL (PNG, JPEG, GIF, WebP or
  AVIF, at most 12 MB). Importing uploads it to the instance in use. Links
  are never followed: a file with an `http(s)` image imports without it.
- `shader` is optional (`null` or missing: none). A theme with a shader
  usually sets `effect` to `custom`; a `custom` effect without a shader reads
  as `none`. A shader that fails the text checks is kept (so it can be fixed)
  but never run, and importing it says so.
- Readers ignore keys they don't know, clamp numbers to the ranges above and
  fall back to defaults for anything else, so a file can't break an app or
  make it load anything from anywhere. `version` is 1; a newer one is read as
  far as it goes, with a note.
