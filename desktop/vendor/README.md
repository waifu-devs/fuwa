# Vendored GPUI

The desktop app draws with GPUI through gpui-kit, which depends on
gpui-pre 0.3.7. Stock GPUI can't do a few things the web app does all the
time, so fuwa keeps its own copy of the crates that needed changing:

- `gpui-pre`: the framework (scene, window, styles, elements).
- `gpui-pre-wgpu`: the Linux renderer (WGSL).
- `gpui-pre-apple`: the macOS renderer (Metal).
- `gpui-pre-macos`: the macOS platform, for its text system (Core Text).
- `gpui-pre-windows`: the Windows renderer (Direct3D 11, HLSL).

They were copied from crates.io unchanged in one commit ("vendor gpui-pre
0.3.7 as published"), so `git diff <that commit> -- desktop/vendor` shows
exactly what fuwa changed. `desktop/Cargo.toml` points at them with
`[patch.crates-io]`. They aren't members of the desktop's workspace, so its
`cargo fmt` and `cargo clippy` leave them alone; `rustfmt.toml` here keeps
Rust's default style for them, as published. Each keeps its license
(`LICENSE-APACHE`).

## What changed

**Rounded clipping.** A content mask can have rounded corners
(`RoundedMask`): a div with `overflow_hidden()` and rounded corners clips its
children to its shape, inside its border, as CSS does. Every primitive's
fragment shader (quads, shadows, underlines, mono and poly sprites, paths,
surfaces) cuts by the mask's rounded-rectangle distance, antialiased. Masks
pushed inside one another are combined into one rounded rectangle
(`RoundedMask::intersect`): the overlap, with each corner keeping the rounding
of whichever mask comes nearest to cutting it. That's exact when an inner mask
shares the outer's corners or is inset the same amount on both sides (a
scroll area inside a bordered card) and close otherwise, erring rounder. The
public `ContentMask` is still a plain rectangle, so other crates are
unaffected; `Window::rounded_content_mask` and `with_rounded_content_mask` are
the rounded side.

**Backdrop blur.** `Styled::backdrop_blur(px)` (and `_sm`, `_md`, `_lg`,
`_xl`, Tailwind's 4, 12, 16 and 24px; plain `backdrop-blur` is 8px) adds a
`BackdropBlur` primitive before the element's background. When a renderer
reaches one in draw order it ends its pass, copies the region under it (with
three standard deviations of margin) out of the frame, shrinks it by 1, 2, 4
or 8 so the gaussian stays 2 to 4 pixels wide, blurs it across and down, and
draws it back inside the element's rounded shape and content mask. Linux reads
the frame back only where the surface allows copying from it (it's skipped
otherwise, leaving the tint); macOS layers are no longer framebuffer-only.

**Transforms.** `Styled::scale`, `scale_x`, `scale_y`, `rotate`,
`translate_x`, `translate_y` and `transform_origin` (CSS's individual
transform properties: scaled, then turned, then moved, around the origin, the
center by default) transform an element and everything painted inside it
without changing layout. `Window::with_transform` keeps a stack of them like
the content mask stack; every primitive carries its element transform and
the vertex shaders apply it, while clipping happens in the element's own
space. Hitboxes remember the transform, so hovering and clicking follow it
(`Hitbox::contains` too). Known limits:

- A content mask from outside a turned element becomes its bounding box inside
  it, so a turned element can draw a little past a parent's clip near its
  corners. Scales and moves are exact.
- Handlers that compare `event.position` with laid-out bounds themselves
  (rather than asking their hitbox) don't see the transform.
- Deferred draws (popovers, tooltips) paint untransformed, as portals do on
  the web.
- Antialiasing widths are in the element's own pixels, which matters only far
  from a scale of 1.

**Synthetic italic.** The app's font (M PLUS Rounded 1c) has no italic face,
and GPUI fell back to the upright one, so `*em*` stood up straight where a
browser slants it. Asked for italic of a family without one, the text systems
now hand out a second font id for the upright face that's drawn leaning 14°,
as browsers do: Linux skews the outline in swash's rasterizer (its image
placement grows to hold it), macOS draws a copy of the CTFont with a skew
matrix and widens the raster bounds by the slant, and Windows asks DirectWrite
to simulate oblique (`DWRITE_FONT_SIMULATIONS_OBLIQUE`), which it didn't
before since layout asked for the upright face's own style. Shaping uses the
upright face, so widths don't change. (`gpui-pre-macos` is vendored for this.)

**Hover transitions.** A div with an id fades its `hover`, `group_hover`
and `active` styles in and out over 150ms (`cubic-bezier(0.4, 0, 0.2, 1)`),
as Tailwind's `transition` does on the web: background colors, border and
text colors, opacity, box shadows and transforms (scale, turn, move) are
mixed between the two styles (colors in premultiplied sRGB, as browsers do),
while everything else switches at once. It remembers each style's fade in
the element's `InteractiveElementState`, starts already there when the
element first shows (CSS doesn't transition on load), asks for frames only
while a fade runs, and settles at once with `App::reduce_motion`. Divs
without an id switch at once, as before.

## Checking the renderers

Only Linux can be built and run here. The wgpu renderer has headless tests
for all three (`cargo test --features test-support --lib` in a copy of
`gpui-pre-wgpu` outside this workspace, patched to `vendor/gpui-pre`, with
Lavapipe). The Metal and HLSL code mirrors the WGSL line for line; on Linux
`gpui-pre-apple` and `gpui-pre-windows` type-check with
`cargo check --target aarch64-apple-darwin` and `x86_64-pc-windows-msvc`, the
HLSL compiles through glslang (`glslc -x hlsl`), and `shaders.metal` parses as
C++ under clang over the cbindgen header. The desktop installers workflow runs
for changes here, so a pull request builds both for real.

## Updating

Copy the new published crates over these, commit that alone, then reapply
the changes above (the diff against the previous "as published" commit is
the patch).
