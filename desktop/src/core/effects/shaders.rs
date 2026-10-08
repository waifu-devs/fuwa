//! The built-in effects as WGSL fragment shaders, word for word the web's
//! `lib/effects/shaders.ts`. Each one is cheap on purpose: a handful of sines
//! and hashes per pixel, no textures, no long loops. They draw in the theme's
//! colors (`c1` primary, `c2` the primary turned 48 degrees, `c3` the page)
//! and give premultiplied alpha.

use crate::core::themes::Effect;

const COMMON: &str = r#"
struct Params {
  c1: vec4f,
  c2: vec4f,
  c3: vec4f,
  res: vec2f,
  time: f32,
  intensity: f32,
}
@group(0) @binding(0) var<uniform> params: Params;

fn hash21(p: vec2f) -> f32 {
  var q = fract(p * vec2f(123.34, 456.21));
  q += dot(q, q + 45.32);
  return fract(q.x * q.y);
}

fn noise(p: vec2f) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash21(i), hash21(i + vec2f(1.0, 0.0)), u.x), mix(hash21(i + vec2f(0.0, 1.0)), hash21(i + vec2f(1.0, 1.0)), u.x), u.y);
}

fn emit(color: vec3f, alpha: f32) -> vec4f {
  let a = clamp(alpha * params.intensity, 0.0, 1.0);
  return vec4f(color * a, a);
}
"#;

const AURORA: &str = r#"
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let aspect = params.res.x / max(params.res.y, 1.0);
  let x = uv.x * aspect;
  let t = params.time * 0.12;
  var color = vec3f(0.0);
  var alpha = 0.0;
  for (var i = 0; i < 3; i++) {
    let fi = f32(i);
    let center = 0.22 + fi * 0.2
      + 0.09 * sin(x * (1.3 + fi * 0.4) + t * (1.0 + fi * 0.35) + fi * 2.1)
      + 0.06 * (noise(vec2f(x * 1.8 + t * 0.8, fi * 3.7)) - 0.5);
    let width = 0.07 + 0.04 * noise(vec2f(x * 2.5 - t, fi + 9.0));
    let d = (uv.y - center) / width;
    // Curtains: bright at the top edge, fading down, with soft vertical streaks.
    let curtain = exp(-d * d) * (0.55 + 0.45 * noise(vec2f(x * 14.0 + t * 2.0, fi)));
    let tint = mix(params.c1.rgb, params.c2.rgb, 0.5 + 0.5 * sin(x * 0.9 + t + fi * 1.9));
    color += tint * curtain;
    alpha += curtain;
  }
  let a = clamp(alpha * 0.9, 0.0, 0.95);
  return emit(color / max(alpha, 0.001), a);
}
"#;

const PETALS: &str = r#"
fn petal(p: vec2f) -> f32 {
  // A cherry blossom petal: a rounded teardrop with a little notch at the tip.
  let q = vec2f(p.x * 1.5, p.y + 0.25 * p.x * p.x);
  let body = length(q) - 0.5;
  let notch = length(p - vec2f(0.0, 0.52)) - 0.13;
  return smoothstep(0.06, -0.02, max(body, -notch));
}

fn layer(uv: vec2f, scale: f32, speed: f32, seed: f32) -> vec2f {
  let aspect = params.res.x / max(params.res.y, 1.0);
  var p = vec2f(uv.x * aspect, uv.y) * scale;
  let t = params.time;
  p.y -= t * speed;
  p.x += sin(t * 0.4 + p.y * 0.6 + seed) * 0.35;
  let cell = floor(p);
  let h = hash21(cell + seed);
  if (h > 0.42) { return vec2f(0.0); }
  let local = fract(p) - 0.5 + (vec2f(hash21(cell + 3.1), hash21(cell + 7.7)) - 0.5) * 0.5;
  let angle = t * (0.6 + h * 1.5) + h * 6.283;
  let flutter = 0.55 + 0.45 * abs(sin(t * (1.0 + h) + h * 9.0));
  let r = mat2x2f(cos(angle), -sin(angle), sin(angle), cos(angle)) * local;
  let shape = petal(vec2f(r.x / flutter, r.y) * 4.2);
  return vec2f(shape, h);
}

@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let near = layer(uv, 4.0, 0.32, 1.0);
  let far = layer(uv, 7.5, 0.2, 17.0);
  let shade = mix(params.c1.rgb, vec3f(1.0), 0.25);
  let a = max(near.x, far.x * 0.7);
  // Far petals are a little deeper in color, like they're in the shade.
  let color = mix(shade * 0.85, shade, step(far.x * 0.7, near.x));
  return emit(color, a);
}
"#;

const STARS: &str = r#"
fn stars(uv: vec2f, scale: f32, seed: f32) -> f32 {
  let aspect = params.res.x / max(params.res.y, 1.0);
  var p = vec2f(uv.x * aspect, uv.y) * scale;
  p.x += params.time * 0.01 * scale;
  let cell = floor(p);
  let h = hash21(cell + seed);
  if (h > 0.22) { return 0.0; }
  let center = vec2f(hash21(cell + 1.3), hash21(cell + 5.9)) * 0.7 + 0.15;
  let d = length(fract(p) - center) * scale;
  let size = 0.06 + h * 0.1;
  let twinkle = 0.55 + 0.45 * sin(params.time * (1.0 + h * 5.0) + h * 40.0);
  let core = smoothstep(size, 0.0, d);
  let glow = smoothstep(size * 6.0, 0.0, d) * 0.4;
  return (core + glow) * twinkle;
}

@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let s = stars(uv, 9.0, 0.0) + stars(uv, 17.0, 11.0) * 0.6 + stars(uv, 31.0, 23.0) * 0.35;
  let nebula = noise(uv * 3.0 + params.time * 0.01) * noise(uv * 5.0 - params.time * 0.008);
  let color = mix(params.c2.rgb, vec3f(1.0), 0.6);
  let haze = mix(params.c1.rgb, params.c2.rgb, uv.x);
  let a = clamp(clamp(s, 0.0, 1.0) + nebula * 0.35, 0.0, 1.0);
  let rgb = (color * clamp(s, 0.0, 1.0) + haze * nebula * 0.35) / max(a, 0.001);
  return emit(rgb, a);
}
"#;

const WAVES: &str = r#"
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let aspect = params.res.x / max(params.res.y, 1.0);
  let x = uv.x * aspect;
  let t = params.time * 0.35;
  var color = vec3f(0.0);
  var alpha = 0.0;
  for (var i = 0; i < 4; i++) {
    let fi = f32(i);
    let height = 0.58 + fi * 0.1
      + 0.035 * sin(x * (2.2 - fi * 0.3) + t * (1.0 + fi * 0.25) + fi * 1.3)
      + 0.02 * sin(x * (5.1 + fi) - t * 1.4 + fi);
    let edge = smoothstep(height - 0.004, height + 0.004, uv.y);
    let a = edge * (0.3 + fi * 0.1);
    let tint = mix(params.c1.rgb, params.c2.rgb, fi / 3.0);
    // Layers stack front over back.
    color = tint * a + color * (1.0 - a);
    alpha = a + alpha * (1.0 - a);
  }
  return emit(color / max(alpha, 0.001), alpha);
}
"#;

/// The fragment shader's own part after `COMMON`, for the effects drawn on the GPU.
fn body(effect: Effect) -> Option<&'static str> {
    match effect {
        Effect::Aurora => Some(AURORA),
        Effect::Petals => Some(PETALS),
        Effect::Stars => Some(STARS),
        Effect::Waves => Some(WAVES),
        _ => None,
    }
}

/// A built-in effect's WGSL, as vgpu's `effect()` gets it on the web.
pub fn source(effect: Effect) -> Option<String> {
    body(effect).map(|b| format!("\n{COMMON}{b}"))
}

/// Smooth effects draw at a lower resolution: they look the same and cost a quarter.
pub fn resolution(effect: Effect) -> f32 {
    match effect {
        Effect::Aurora => 0.5,
        Effect::Waves => 0.75,
        _ => 1.0,
    }
}

/// vgpu's full-screen triangle, which the web puts in front of every
/// effect. Here it goes after, so a custom shader's lines keep their numbers.
pub const VERTEX: &str = r#"
struct VgpuFullscreenVertexOut {
  @builtin(position) position: vec4f,
  @location(0) uv: vec2f,
};
@vertex fn vgpu_fullscreen_vs(@builtin(vertex_index) vi: u32) -> VgpuFullscreenVertexOut {
  var pos = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  var uv = array<vec2f, 3>(vec2f(0.0, 1.0), vec2f(2.0, 1.0), vec2f(0.0, -1.0));
  var out: VgpuFullscreenVertexOut;
  out.position = vec4f(pos[vi], 0.0, 1.0);
  out.uv = uv[vi];
  return out;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shaders_are_the_webs_word_for_word() {
        let path = format!("{}/../web/src/lib/effects/shaders.ts", env!("CARGO_MANIFEST_DIR"));
        let web = std::fs::read_to_string(path).expect("the web app's shaders");
        assert!(web.contains(&format!("const COMMON = /* wgsl */ `{COMMON}`;")));
        for (name, effect) in
            [("AURORA", Effect::Aurora), ("PETALS", Effect::Petals), ("STARS", Effect::Stars), ("WAVES", Effect::Waves)]
        {
            let body = body(effect).unwrap();
            assert!(web.contains(&format!("const {name} = /* wgsl */ `\n${{COMMON}}{body}`;")), "{name}");
        }
        assert!(
            web.contains(
                "RESOLUTION: Record<ShaderEffect, number> = { aurora: 0.5, petals: 1, stars: 1, waves: 0.75 }"
            )
        );
        assert!(source(Effect::Grid).is_none());
    }
}
