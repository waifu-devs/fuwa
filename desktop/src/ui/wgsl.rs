//! WGSL in colors for the custom shader editor, the web's `highlight` in
//! `settings/app/ShaderEditor.tsx`: comments, numbers, keywords, types and
//! fuwa's own names, found by the same rules.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, InputHighlighterFactory, Rope,
};
use gpui_kit::{Context, FontStyle, FontWeight, HighlightStyle, Hsla, SharedString, Window, rgb};

use crate::ui::theme::{Palette, mix};

const KEYWORDS: [&str; 18] = [
    "fn", "let", "var", "const", "return", "if", "else", "for", "loop", "while", "break", "continue", "struct",
    "switch", "case", "default", "true", "false",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Comment,
    Number,
    Keyword,
    Type,
    /// `fuwa`, `shade` and `uv`.
    Name,
}

fn is_type(word: &str) -> bool {
    let b = word.as_bytes();
    let size = |c: u8| (b'2'..=b'4').contains(&c);
    match word {
        "f32" | "i32" | "u32" | "bool" | "array" => true,
        _ if word.starts_with("vec") => {
            (b.len() == 4 && size(b[3])) || (b.len() == 5 && size(b[3]) && matches!(b[4], b'f' | b'i' | b'u'))
        }
        _ if word.starts_with("mat") => {
            (b.len() == 6 || (b.len() == 7 && b[6] == b'f')) && size(b[3]) && b[4] == b'x' && size(b[5])
        }
        _ => false,
    }
}

fn word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The colored pieces of `code`, in order, as byte ranges.
pub fn tokens(code: &str) -> Vec<(Range<usize>, Kind)> {
    let b = code.as_bytes();
    let digits = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        i
    };
    // An exponent (`e`, an optional sign, digits) at `i`, or nothing.
    let exponent = |i: usize| {
        if i < b.len() && b[i] == b'e' {
            let j = if i + 1 < b.len() && matches!(b[i + 1], b'+' | b'-') { i + 2 } else { i + 1 };
            if j < b.len() && b[j].is_ascii_digit() {
                return digits(j);
            }
        }
        i
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if code[i..].starts_with("//") {
            i = code[i..].find('\n').map_or(b.len(), |n| i + n);
            out.push((start..i, Kind::Comment));
        } else if code[i..].starts_with("/*") {
            i = code[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
            out.push((start..i, Kind::Comment));
        } else if c.is_ascii_digit() || (c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            if c == b'.' {
                i = exponent(digits(i + 1));
                if i < b.len() && b[i] == b'f' {
                    i += 1;
                }
            } else {
                i = digits(i);
                if i < b.len() && b[i] == b'.' {
                    i = digits(i + 1);
                }
                i = exponent(i);
                if i < b.len() && matches!(b[i], b'f' | b'i' | b'u') {
                    i += 1;
                }
            }
            out.push((start..i, Kind::Number));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && word_char(b[i]) {
                i += 1;
            }
            let word = &code[start..i];
            let kind = if KEYWORDS.contains(&word) {
                Some(Kind::Keyword)
            } else if is_type(word) {
                Some(Kind::Type)
            } else if matches!(word, "fuwa" | "shade" | "uv") {
                Some(Kind::Name)
            } else {
                None
            };
            if let Some(kind) = kind {
                out.push((start..i, kind));
            }
        } else {
            // Past the whole character, so a slice never lands inside one.
            i += code[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// The colors each kind takes, from the theme on screen.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Colors {
    comment: Hsla,
    number: Hsla,
    keyword: Hsla,
    kind: Hsla,
    name: Hsla,
}

impl Colors {
    /// The web's `--wgsl-number` and `--wgsl-type`: an orange and a teal leaning to the theme's primary.
    pub fn of(p: &Palette) -> Self {
        Colors {
            comment: p.muted_foreground.into(),
            number: mix(rgb(0xe8962e), p.primary, 0.35),
            keyword: p.primary.into(),
            kind: mix(rgb(0x1fa898), p.primary, 0.35),
            name: p.foreground.into(),
        }
    }

    fn style(&self, kind: Kind) -> HighlightStyle {
        let (color, weight, italic) = match kind {
            Kind::Comment => (self.comment, None, true),
            Kind::Number => (self.number, None, false),
            Kind::Keyword => (self.keyword, Some(FontWeight::BOLD), false),
            Kind::Type => (self.kind, None, false),
            Kind::Name => (self.name, Some(FontWeight::BOLD), false),
        };
        HighlightStyle {
            color: Some(color),
            font_weight: weight,
            font_style: italic.then_some(FontStyle::Italic),
            ..Default::default()
        }
    }
}

/// The editor's highlighter, reading its colors from `colors`, which the
/// settings page keeps in step with the theme.
struct Wgsl {
    colors: Rc<Cell<Colors>>,
    tokens: Vec<(Range<usize>, Kind)>,
}

impl InputHighlighter for Wgsl {
    fn language(&self) -> SharedString {
        "wgsl".into()
    }

    fn update(&mut self, _: Option<InputEdit>, text: &Rope, _: bool, _: &mut Window, _: &mut Context<EditorState>) {
        // Shaders are small (MAX_SHADER_BYTES), so the whole text each time.
        self.tokens = tokens(&text.to_string());
    }

    fn styles(&self, range: &Range<usize>, _: &dyn HighlightStyleResolver) -> Vec<(Range<usize>, HighlightStyle)> {
        let colors = self.colors.get();
        let mut out = Vec::new();
        let mut at = range.start;
        let first = self.tokens.partition_point(|(r, _)| r.end <= range.start);
        for (r, kind) in &self.tokens[first..] {
            if r.start >= range.end {
                break;
            }
            let (start, end) = (r.start.max(at), r.end.min(range.end));
            if start > at {
                out.push((at..start, HighlightStyle::default()));
            }
            out.push((start..end, colors.style(*kind)));
            at = end;
        }
        if at < range.end {
            out.push((at..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

/// A factory for the shader editor's highlighter, whatever language it's set to.
pub fn factory(colors: Rc<Cell<Colors>>) -> InputHighlighterFactory {
    Rc::new(move |_| Some(Box::new(Wgsl { colors: colors.clone(), tokens: Vec::new() }) as Box<dyn InputHighlighter>))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(code: &str) -> Vec<(&str, Kind)> {
        tokens(code).into_iter().map(|(r, k)| (&code[r], k)).collect()
    }

    #[test]
    fn finds_what_the_web_colors() {
        assert_eq!(
            kinds(
                "fn shade(uv: vec2f) -> vec4f {\n  let t = fuwa.time * 0.04; // slow\n  return vec4f(t, 1.5e-3, .5, 2u);\n}"
            ),
            vec![
                ("fn", Kind::Keyword),
                ("shade", Kind::Name),
                ("uv", Kind::Name),
                ("vec2f", Kind::Type),
                ("vec4f", Kind::Type),
                ("let", Kind::Keyword),
                ("fuwa", Kind::Name),
                ("0.04", Kind::Number),
                ("// slow", Kind::Comment),
                ("return", Kind::Keyword),
                ("vec4f", Kind::Type),
                ("1.5e-3", Kind::Number),
                (".5", Kind::Number),
                ("2u", Kind::Number),
            ]
        );
    }

    #[test]
    fn words_with_digits_stay_words() {
        assert_eq!(kinds("hash21(p2) mat3x3f mat2x5 vec5"), vec![("mat3x3f", Kind::Type)]);
    }

    #[test]
    fn block_comments_run_to_their_end_or_the_text_end() {
        assert_eq!(kinds("/* a\nb */ x /* open"), vec![("/* a\nb */", Kind::Comment), ("/* open", Kind::Comment)]);
    }

    #[test]
    fn other_characters_are_stepped_over_whole() {
        assert_eq!(
            kinds("// é\nlet é = 1;"),
            vec![("// é", Kind::Comment), ("let", Kind::Keyword), ("1", Kind::Number)]
        );
    }
}
