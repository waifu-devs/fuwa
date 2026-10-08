//! Custom shaders, as the web's `lib/effects/custom.ts` (docs/themes.md,
//! Custom shaders): effects people write themselves, in WGSL, kept in a
//! backdrop (and so in theme files). Someone writes one function,
//!
//!   fn shade(uv: vec2f) -> vec4f
//!
//! which returns a color and how much of it covers what's behind (straight
//! alpha, 0 to 1). fuwa puts it between its own prelude and entry point,
//! word for word the web's, so a shader draws the same in both apps. The
//! shader can't declare anything the app doesn't give it: attributes are
//! refused, so it has no textures, no buffers and nothing from anywhere but
//! these inputs.
//!
//! A shader that doesn't pass these checks, doesn't compile, is too slow or
//! stops the GPU never breaks the app: the backdrop shows its fallback.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::i18n::{Arg, t, t_with};
use crate::core::themes::Effect;

/// At most this many bytes of WGSL (UTF-8).
pub const MAX_SHADER_BYTES: usize = 16 * 1024;
pub const SHADER_NAME_MAX: usize = 32;
/// What can show where a shader can't run.
pub const FALLBACKS: [Effect; 5] = [Effect::None, Effect::Aurora, Effect::Petals, Effect::Stars, Effect::Waves];

/// A shader someone wrote, kept in a backdrop's `shader`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CustomShader {
    /// What it's called on the effect card.
    pub name: String,
    /// The WGSL, defining `fn shade(uv: vec2f) -> vec4f`.
    pub code: String,
    /// What shows where it can't run.
    pub fallback: Effect,
}

impl<'de> Deserialize<'de> for CustomShader {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        sanitize_shader(&Value::deserialize(d)?).ok_or_else(|| serde::de::Error::custom("a shader has code"))
    }
}

/// The inputs, as WGSL. The web's `INPUTS`, kept in sync with docs/themes.md.
const INPUTS: &str = r#"struct Fuwa {
  primary: vec4f,
  accent: vec4f,
  background: vec4f,
  foreground: vec4f,
  res: vec2f,
  pointer: vec2f,
  time: f32,
  intensity: f32,
}
@group(0) @binding(0) var<uniform> fuwa: Fuwa;

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

fn fbm(p: vec2f) -> f32 {
  var v = 0.0;
  var a = 0.5;
  var q = p;
  for (var i = 0; i < 4; i++) {
    v += a * noise(q);
    q = q * 2.03 + vec2f(1.7, 9.2);
    a *= 0.5;
  }
  return v;
}
"#;

const MAIN: &str = r#"
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let c = shade(uv);
  let a = clamp(c.a * fuwa.intensity, 0.0, 1.0);
  return vec4f(clamp(c.rgb, vec3f(0.0), vec3f(1.0)) * a, a);
}
"#;

/// Lines before the shader's own first line in the full source, to point errors at the right line.
pub fn prelude_lines() -> usize {
    INPUTS.split('\n').count()
}

/// The whole WGSL module for a shader.
pub fn full_source(code: &str) -> String {
    format!("{INPUTS}\n{code}\n{MAIN}")
}

/// The shader's line for a line of the full source, or None when it's in fuwa's own part.
pub fn shader_line(full: usize, code: &str) -> Option<usize> {
    let line = full.checked_sub(prelude_lines())?;
    (line >= 1 && line <= code.split('\n').count()).then_some(line)
}

/// WGSL's line breaks: a line comment ends at any of them.
fn line_break(c: char) -> bool {
    matches!(c, '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The code without its comments, read left to right as the WGSL compiler
/// reads it: `//` runs to the next line break, and block comments nest.
pub fn strip_comments(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("//") {
            rest = &rest[2..];
            let end = rest.find(line_break).unwrap_or(rest.len());
            rest = &rest[end..];
            continue;
        }
        if rest.starts_with("/*") {
            let mut depth = 1;
            rest = &rest[2..];
            while depth > 0 && !rest.is_empty() {
                if rest.starts_with("/*") {
                    depth += 1;
                    rest = &rest[2..];
                } else if rest.starts_with("*/") {
                    depth -= 1;
                    rest = &rest[2..];
                } else {
                    let n = rest.chars().next().map_or(1, char::len_utf8);
                    rest = &rest[n..];
                }
            }
            out.push(' ');
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Where `word` stands as a whole word (JavaScript's `\b`, ASCII word characters).
fn words<'a>(text: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(word).map(|(at, _)| at).filter(move |&at| {
        let before = text[..at].chars().next_back().is_none_or(|c| !word_char(c));
        let after = text[at + word.len()..].chars().next().is_none_or(|c| !word_char(c));
        before && after
    })
}

/// A link anywhere: `scheme://`, a scheme being a letter and then letters, digits, `+`, `.` or `-`.
fn has_link(code: &str) -> bool {
    code.match_indices("://").any(|(at, _)| {
        code[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
            .any(|c| c.is_ascii_alphabetic())
    })
}

/// `fn shade(`, with any spacing between.
fn has_shade(live: &str) -> bool {
    words(live, "fn").any(|at| {
        let rest = &live[at + 2..];
        let name = rest.trim_start();
        name.len() < rest.len() && name.strip_prefix("shade").is_some_and(|r| r.trim_start().starts_with('('))
    })
}

/// What's wrong with a shader before it goes near a GPU, or None. These
/// keep it to the app's inputs; everything else is the compiler's to say.
pub fn shader_problem(code: &str) -> Option<String> {
    if code.trim().is_empty() {
        return Some(t("system.shader.empty"));
    }
    if code.len() > MAX_SHADER_BYTES {
        return Some(t_with("system.shader.tooBig", &[("kb", Arg::Num((MAX_SHADER_BYTES / 1024) as i64))]));
    }
    // Links, even in comments: there's nothing for a shader to point at.
    if has_link(code) {
        return Some(t("system.shader.links"));
    }
    let live = strip_comments(code);
    if live.contains('@') {
        return Some(t("system.shader.attributes"));
    }
    // Reserved words in WGSL, so outside a comment they're the directives.
    if ["enable", "requires", "diagnostic"].iter().any(|w| words(&live, w).next().is_some()) {
        return Some(t("system.shader.extensions"));
    }
    if !has_shade(&live) {
        return Some(t("system.shader.noShade"));
    }
    None
}

/// A shader as stored or imported: text that can't break anything, its size
/// capped. Its code may still have problems (`shader_problem`).
pub fn sanitize_shader(value: &Value) -> Option<CustomShader> {
    let s = value.as_object()?;
    let code = s.get("code")?.as_str()?;
    // Line ends made plain, and no control characters but tabs and line ends.
    let mut code: String = code
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|&c| !matches!(c, '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'))
        .collect();
    if code.len() > MAX_SHADER_BYTES {
        let mut end = MAX_SHADER_BYTES;
        while !code.is_char_boundary(end) {
            end -= 1;
        }
        code.truncate(end);
    }
    let name: String = s
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() && (c as u32) < 0x80 { ' ' } else { c })
        .collect();
    let name: String = name.trim().chars().take(SHADER_NAME_MAX).collect();
    let fallback = s
        .get("fallback")
        .and_then(Value::as_str)
        .and_then(|f| FALLBACKS.into_iter().find(|e| e.key() == f))
        .unwrap_or(Effect::Aurora);
    Some(CustomShader { name: if name.is_empty() { "My shader".into() } else { name }, code, fallback })
}

/// A short, stable id for a shader's code (FNV-1a over UTF-16, like the web's), to remember what happened to it.
pub fn shader_id(code: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for unit in code.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
    }
    let mut digits = Vec::new();
    loop {
        let d = (h % 36) as u8;
        digits.push(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        h /= 36;
        if h == 0 {
            break;
        }
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

// ───────────────────────── Starters ─────────────────────────

/// A shader to start from: its id, name and the catalog key of its hint.
pub struct Starter {
    pub id: &'static str,
    pub name: &'static str,
    pub hint: &'static str,
    pub fallback: Effect,
    pub code: &'static str,
}

impl Starter {
    pub fn shader(&self) -> CustomShader {
        CustomShader { name: self.name.to_owned(), code: self.code.to_owned(), fallback: self.fallback }
    }
}

/// The web's `STARTERS`, each a small lesson in the inputs.
pub const STARTERS: [Starter; 4] = [
    Starter {
        id: "glow",
        name: "Glow",
        hint: "appsettings.shader.starter.glow",
        fallback: Effect::Aurora,
        code: r#"// A soft light that follows the pointer, breathing slowly.
fn shade(uv: vec2f) -> vec4f {
  let aspect = fuwa.res.x / fuwa.res.y;
  let d = (uv - fuwa.pointer) * vec2f(aspect, 1.0);
  let breathe = 0.85 + 0.15 * sin(fuwa.time * 1.2);
  let glow = exp(-dot(d, d) * 9.0) * breathe;
  let color = mix(fuwa.accent.rgb, fuwa.primary.rgb, glow);
  return vec4f(color, glow * 0.8);
}
"#,
    },
    Starter {
        id: "plasma",
        name: "Plasma",
        hint: "appsettings.shader.starter.plasma",
        fallback: Effect::Aurora,
        code: r#"// Melting color fields in the theme's primary and accent.
fn shade(uv: vec2f) -> vec4f {
  let p = uv * vec2f(fuwa.res.x / fuwa.res.y, 1.0) * 4.0;
  let t = fuwa.time * 0.3;
  let v = sin(p.x + t) + sin(p.y * 1.3 - t) + sin((p.x + p.y) * 0.7 + t * 1.7)
    + sin(length(p - vec2f(2.0, 1.5)) * 1.8 - t * 2.0);
  let k = 0.5 + 0.5 * sin(v * 1.4);
  let color = mix(fuwa.primary.rgb, fuwa.accent.rgb, k);
  return vec4f(color, 0.4 + 0.4 * k);
}
"#,
    },
    Starter {
        id: "ripples",
        name: "Ripples",
        hint: "appsettings.shader.starter.ripples",
        fallback: Effect::Waves,
        code: r#"// Rings spreading out from the pointer, fading as they go.
fn shade(uv: vec2f) -> vec4f {
  let aspect = fuwa.res.x / fuwa.res.y;
  let d = length((uv - fuwa.pointer) * vec2f(aspect, 1.0));
  let rings = 0.5 + 0.5 * sin(d * 40.0 - fuwa.time * 3.0);
  let fade = exp(-d * 3.0);
  let line = smoothstep(0.85, 1.0, rings) * fade;
  return vec4f(mix(fuwa.primary.rgb, vec3f(1.0), 0.3), line);
}
"#,
    },
    Starter {
        id: "clouds",
        name: "Clouds",
        hint: "appsettings.shader.starter.clouds",
        fallback: Effect::Aurora,
        code: r#"// Clouds drifting by, made with fbm (layered noise).
fn shade(uv: vec2f) -> vec4f {
  let p = uv * vec2f(fuwa.res.x / fuwa.res.y, 1.0) * 2.5;
  let t = fuwa.time * 0.04;
  let n = fbm(p + vec2f(t * 3.0, t) + fbm(p * 1.5 - t) * 0.6);
  let cloud = smoothstep(0.42, 0.78, n);
  // Pale at the tops, the accent in the shade underneath.
  let color = mix(fuwa.accent.rgb, mix(fuwa.primary.rgb, vec3f(1.0), 0.7), cloud);
  return vec4f(color, cloud * 0.75);
}
"#,
    },
];

/// The shader a new custom effect starts with.
pub fn default_shader() -> CustomShader {
    STARTERS[0].shader()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADE: &str = "fn shade(uv: vec2f) -> vec4f { return vec4f(0.0); }";

    fn web(file: &str) -> String {
        let path = format!("{}/../web/src/lib/effects/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).expect("the web app's effects")
    }

    #[test]
    fn the_prelude_and_starters_are_the_webs_word_for_word() {
        let custom = web("custom.ts");
        assert!(custom.contains(&format!("const INPUTS = /* wgsl */ `{INPUTS}`;")));
        assert!(custom.contains(&format!("const MAIN = /* wgsl */ `{MAIN}`;")));
        for s in &STARTERS {
            assert!(custom.contains(&format!("code: `{}`", s.code)), "{}", s.id);
            assert!(custom.contains(&format!("fallback: \"{}\"", s.fallback.key())), "{}", s.id);
            assert!(
                custom.contains(&format!("id: \"{}\",\n    name: \"{}\",\n    hint: \"{}\"", s.id, s.name, s.hint))
            );
        }
    }

    #[test]
    fn the_starters_pass_the_checks() {
        for s in &STARTERS {
            assert_eq!(shader_problem(s.code), None, "{}", s.id);
        }
    }

    #[test]
    fn comments_are_read_as_the_compiler_reads_them() {
        let attributes = t("system.shader.attributes");
        let code = format!("// /*\n@group(0) @binding(1) var<storage, read_write> x: array<f32>;\n// */\n{SHADE}");
        assert_eq!(shader_problem(&code), Some(attributes.clone()));
        for br in ["\n", "\r", "\u{b}", "\u{c}", "\u{85}", "\u{2028}", "\u{2029}"] {
            let code = format!("// note{br}@group(0) @binding(1) var<uniform> y: f32;\n{SHADE}");
            assert_eq!(shader_problem(&code), Some(attributes.clone()), "{br:?}");
        }
        assert_eq!(strip_comments("a /* b /* c */ d */ e"), "a   e");
        assert_eq!(shader_problem(&format!("/* /* */ {SHADE}")), Some(t("system.shader.noShade")));
        assert_eq!(shader_problem(&format!("// @group /* enable\n/* @binding requires */\n{SHADE}")), None);
    }

    #[test]
    fn links_extensions_and_attributes_are_refused() {
        assert_eq!(shader_problem(&format!("// from https://example.com\n{SHADE}")), Some(t("system.shader.links")));
        assert_eq!(shader_problem(&format!("enable f16;\n{SHADE}")), Some(t("system.shader.extensions")));
        assert_eq!(shader_problem(&format!("@must_use {SHADE}")), Some(t("system.shader.attributes")));
        assert_eq!(shader_problem("fn other() {}"), Some(t("system.shader.noShade")));
        assert_eq!(shader_problem("fn  shade  (uv: vec2f) -> vec4f { return vec4f(0.0); }"), None);
        assert_eq!(
            shader_problem("fn shader(uv: vec2f) -> vec4f { return vec4f(0.0); }"),
            Some(t("system.shader.noShade"))
        );
        assert_eq!(shader_problem("let renabled = 1.0;\nfn shade(uv: vec2f) -> vec4f { return vec4f(0.0); }"), None);
        assert_eq!(shader_problem(" \n"), Some(t("system.shader.empty")));
    }

    #[test]
    fn stored_shaders_are_capped_and_lose_control_characters() {
        let code = format!("{SHADE}\r\n\u{1}{}", "é".repeat(20000));
        let s = sanitize_shader(&serde_json::json!({ "name": "x\u{7}y", "code": code, "fallback": "nope" })).unwrap();
        assert!(s.code.len() <= MAX_SHADER_BYTES);
        assert!(!s.code.contains(['\r', '\u{1}']));
        assert_eq!((s.name.as_str(), s.fallback), ("x y", Effect::Aurora));
        let s = sanitize_shader(&serde_json::json!({ "code": SHADE, "fallback": "none" })).unwrap();
        assert_eq!((s.name.as_str(), s.fallback), ("My shader", Effect::None));
        assert!(sanitize_shader(&serde_json::json!({ "fallback": "stars" })).is_none());
        // Only the shader effects can stand in.
        let s = sanitize_shader(&serde_json::json!({ "code": SHADE, "fallback": "grid" })).unwrap();
        assert_eq!(s.fallback, Effect::Aurora);
    }

    #[test]
    fn ids_match_the_webs() {
        // FNV-1a's offset basis, then one step for "a", in base 36 like `toString(36)`.
        assert_eq!(shader_id(""), radix(0x811c_9dc5));
        assert_eq!(shader_id("a"), radix((0x811c_9dc5u32 ^ 0x61).wrapping_mul(0x0100_0193)));
        assert_ne!(shader_id(STARTERS[0].code), shader_id(STARTERS[1].code));
    }

    fn radix(mut n: u32) -> String {
        let mut out = String::new();
        loop {
            out.insert(0, char::from_digit(n % 36, 36).unwrap());
            n /= 36;
            if n == 0 {
                return out;
            }
        }
    }

    #[test]
    fn errors_point_at_the_shaders_own_lines() {
        let code = "a\nb\nc";
        let first = prelude_lines() + 1;
        assert_eq!(shader_line(first, code), Some(1));
        assert_eq!(shader_line(first + 2, code), Some(3));
        assert_eq!(shader_line(first + 3, code), None);
        assert_eq!(shader_line(3, code), None);
        assert_eq!(full_source(code).lines().nth(first - 1), Some("a"));
    }
}
