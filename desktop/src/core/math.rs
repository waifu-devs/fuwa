//! Math in Markdown: `$inline$`, `$$display$$` and ```math / ```latex code
//! blocks (the web's `lib/math.ts`, rule for rule, so a message reads the
//! same in both apps), drawn by `ui/math.rs`.
//!
//! Which dollars are math is decided on the source, before anything else
//! reads it, so prices stay prices: a single `$` opens only when a non-space
//! follows it and closes only after a non-space and before anything but a
//! digit, on the same line ("$5 and $10" is text, "$x^2$" is math). `$$` pairs
//! may run over lines but not over a blank one. Code, fenced or inline, and
//! `\$` are left alone.
//!
//! Each formula found becomes an inline code span on one line whose text
//! starts with a private-use mark, so every later step (spoilers, mentions,
//! timestamps, search hits) treats it as code and leaves its insides alone,
//! and the Markdown plugins in `ui/math.rs` draw the marked spans. A `$$` pair
//! alone in its paragraph is a display formula, on a line of its own; one
//! amid text is drawn inline in display style. Unpaired `$$` are escaped, so
//! the parser never reads them as a block that runs to the end of the message.

/// Starts an inline formula's code span.
pub const INLINE_MARK: char = '\u{E0F1}';
/// Starts a `$$` formula's code span.
pub const DISPLAY_MARK: char = '\u{E0F2}';

/// The marks above, and search's match marks (`ui/search.rs`), which never belong in TeX.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{E000}' | '\u{E001}' | INLINE_MARK | DISPLAY_MARK)
}

/// The fence a line opens or closes (` ``` ` or `~~~`, after any quote marks).
fn fence_of(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches([' ', '\t', '>']);
    let ch = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = rest.chars().take_while(|c| *c == ch).count();
    (n >= 3).then(|| &rest[..n])
}

/// The source with every formula made a marked code span (see above).
pub fn mark(source: &str) -> String {
    if !source.contains('$') {
        return source.to_owned();
    }
    let mut out: Vec<String> = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let mut fence: Option<String> = None;
    for line in source.split('\n') {
        let opens = fence_of(line);
        if let Some(open) = &fence {
            out.push(line.to_owned());
            if opens.is_some_and(|f| f.starts_with(&open[..1]) && f.len() >= open.len()) {
                fence = None;
            }
        } else if let Some(f) = opens {
            if !text.is_empty() {
                out.push(scan(&text.join("\n")));
                text.clear();
            }
            fence = Some(f.to_owned());
            out.push(line.to_owned());
        } else {
            text.push(line);
        }
    }
    if !text.is_empty() {
        out.push(scan(&text.join("\n")));
    }
    out.join("\n")
}

/// How many `ch` in a row start at `at`.
fn run_at(s: &[char], at: usize, ch: char) -> usize {
    s[at.min(s.len())..].iter().take_while(|c| **c == ch).count()
}

/// Where the next run of exactly `n` `ch` starts after `from`; `$` stops at
/// the line's end (`line`) or a blank line.
fn closing(s: &[char], from: usize, ch: char, n: usize, line: bool) -> Option<usize> {
    let mut i = from;
    while i < s.len() {
        let c = s[i];
        if c == '\n' {
            if line && ch == '$' {
                return None;
            }
            let end = s[i + 1..].iter().position(|c| *c == '\n').map_or(s.len(), |p| i + 1 + p);
            if s[i + 1..end].iter().all(|c| matches!(c, ' ' | '\t' | '>')) {
                return None;
            }
        } else if ch == '$' && c == '\\' && s.get(i + 1) == Some(&'$') {
            // `\$` is a dollar sign in TeX, not the end.
            i += 2;
            continue;
        } else if c == ch {
            let run = run_at(s, i, ch);
            if run == n {
                return Some(i);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

/// How many `>` quote the line `at` is on.
fn quote_depth(s: &[char], at: usize) -> usize {
    let start = s[..at].iter().rposition(|c| *c == '\n').map_or(0, |p| p + 1);
    s[start..at].iter().take_while(|c| matches!(c, ' ' | '\t' | '>')).filter(|c| **c == '>').count()
}

/// A formula's TeX on one line: quote marks of its lines and `%` comments
/// taken out, spaces run together.
fn tex_of(content: &[char], depth: usize) -> String {
    let content: String = content.iter().filter(|c| !is_mark(**c)).collect();
    let mut joined = String::with_capacity(content.len());
    for (n, mut line) in content.split('\n').enumerate() {
        if n > 0 {
            joined.push(' ');
            for _ in 0..depth {
                let rest = line.trim_start_matches([' ', '\t']);
                match rest.strip_prefix('>') {
                    Some(after) => line = after,
                    None => break,
                }
            }
        }
        let comment = line.char_indices().find(|&(i, c)| c == '%' && !line[..i].ends_with('\\'));
        joined.push_str(comment.map_or(line, |(i, _)| &line[..i]));
    }
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A formula as a marked code span, its fence longer than any backticks inside.
fn span(tex: &str, display: bool) -> String {
    let longest = tex.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let mark = if display { DISPLAY_MARK } else { INLINE_MARK };
    let pad = if tex.ends_with('`') { " " } else { "" };
    format!("{fence}{mark}{tex}{pad}{fence}")
}

/// One stretch of text between code blocks, its formulas marked.
fn scan(text: &str) -> String {
    let s: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == '\\' && s.get(i + 1).is_some_and(char::is_ascii_punctuation) {
            out.extend(&s[i..i + 2]);
            i += 2;
        } else if c == '`' {
            let n = run_at(&s, i, '`');
            let to = closing(&s, i + n, '`', n, false).map_or(i + n, |end| end + n);
            out.extend(&s[i..to]);
            i = to;
        } else if c == '$' {
            let n = run_at(&s, i, '$');
            let end = if n <= 2 { closing(&s, i + n, '$', n, n == 1) } else { None };
            let tex = end
                .filter(|&end| {
                    let content = &s[i + n..end];
                    n == 2
                        || (content.first().is_some_and(|c| !c.is_whitespace())
                            && content.last().is_some_and(|c| !c.is_whitespace())
                            && !s.get(end + 1).is_some_and(char::is_ascii_digit))
                })
                .map(|end| tex_of(&s[i + n..end], quote_depth(&s, i)))
                .filter(|tex| !tex.is_empty());
            match (tex, end) {
                (Some(tex), Some(end)) => {
                    out.push_str(&span(&tex, n == 2));
                    i = end + n;
                }
                _ => {
                    if n == 1 {
                        out.push('$');
                    } else {
                        out.push_str(&"\\$".repeat(n));
                    }
                    i += n;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The most TeX a formula may have and still be drawn.
pub const MAX_TEX: usize = 2000;

/// Commands that define commands: a formula using one isn't drawn, so
/// nobody's message runs macros of its own (the web's `DEFINES`).
const DEFINES: &[&str] = &[
    "def",
    "edef",
    "gdef",
    "xdef",
    "let",
    "futurelet",
    "global",
    "newcommand",
    "renewcommand",
    "providecommand",
    "newenvironment",
    "renewenvironment",
    "DeclareMathOperator",
    "csname",
    "expandafter",
];

/// Whether a formula may be drawn: not too long, and defining no commands.
pub fn allowed(tex: &str) -> bool {
    if tex.chars().count() > MAX_TEX {
        return false;
    }
    let mut rest = tex;
    while let Some(at) = rest.find('\\') {
        rest = &rest[at + 1..];
        let name: &str = &rest[..rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len())];
        if DEFINES.contains(&name) {
            return false;
        }
    }
    true
}

/// A formula a marked code span holds (`mark` made it): its TeX, and whether it was `$$`.
pub fn formula(code: &str) -> Option<(&str, bool)> {
    let display = code.starts_with(DISPLAY_MARK);
    let tex = code.strip_prefix(INLINE_MARK).or_else(|| code.strip_prefix(DISPLAY_MARK))?.trim();
    (!tex.is_empty()).then_some((tex, display))
}

/// Whether a fenced code block's language is drawn as math.
pub fn is_math_lang(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("math") || lang.eq_ignore_ascii_case("latex")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The same cases are in the web's lib/math.test.ts: both apps must agree on what's math.
    const I: char = INLINE_MARK;
    const D: char = DISPLAY_MARK;

    #[test]
    fn formulas_become_marked_code_spans() {
        assert_eq!(mark("so $x^2$ then"), format!("so `{I}x^2` then"));
        assert_eq!(mark("$$\\sum_i i$$"), format!("`{D}\\sum_i i`"));
        assert_eq!(mark("a $x$ and $y$"), format!("a `{I}x` and `{I}y`"));
    }

    #[test]
    fn prices_stay_text() {
        for text in
            ["costs $5 and $10", "$5-$10", "between $5 and $6.50", "US$5 vs US$6", "it's $ 5 or $ 6", "$5/$10 a month"]
        {
            assert_eq!(mark(text), text);
        }
        assert_eq!(mark("I have $5"), "I have $5");
    }

    #[test]
    fn single_dollars_keep_to_one_line_double_ones_to_one_paragraph() {
        assert_eq!(mark("$a\nb$"), "$a\nb$");
        assert_eq!(mark("$$\na + b\n$$"), format!("`{D}a + b`"));
        assert_eq!(mark("$$a\n\nb$$"), "\\$\\$a\n\nb\\$\\$");
    }

    #[test]
    fn code_and_escapes_are_left_alone() {
        assert_eq!(mark("`$x$` and \\$5 and \\$x$"), "`$x$` and \\$5 and \\$x$");
        assert_eq!(mark("```\n$x$\n```\n$y$"), format!("```\n$x$\n```\n`{I}y`"));
        assert_eq!(mark("~~~tex\n$$a$$\n~~~"), "~~~tex\n$$a$$\n~~~");
    }

    #[test]
    fn a_formulas_lines_join_without_quote_marks_or_comments() {
        assert_eq!(mark("> $$\n> a % note\n> + b\n> $$"), format!("> `{D}a + b`"));
    }

    #[test]
    fn backticks_inside_a_formula_get_a_longer_fence() {
        assert_eq!(mark("$a`b$"), format!("``{I}a`b``"));
    }

    #[test]
    fn search_marks_never_reach_the_tex() {
        assert_eq!(mark("$\u{E000}x\u{E001}$"), format!("`{I}x`"));
    }

    #[test]
    fn unpaired_double_dollars_are_escaped_single_ones_stay() {
        assert_eq!(mark("$$ and $"), "\\$\\$ and $");
        assert_eq!(mark("$$$x$$$"), "\\$\\$\\$x\\$\\$\\$");
    }

    #[test]
    fn formulas_that_define_commands_arent_drawn() {
        assert!(allowed("\\frac{a}{b}"));
        assert!(allowed("\\deff"));
        assert!(!allowed("\\def\\x{y}\\x"));
        assert!(!allowed("\\newcommand{\\x}{y}"));
        assert!(!allowed("\\gdef\\x{y}"));
        assert!(!allowed(&"x".repeat(2001)));
    }

    #[test]
    fn marked_spans_read_back() {
        assert_eq!(formula(&format!("{I}x ")), Some(("x", false)));
        assert_eq!(formula(&format!("{D}y")), Some(("y", true)));
        assert_eq!(formula("plain"), None);
        assert_eq!(formula(&format!("{I} ")), None);
        assert!(is_math_lang("LaTeX") && is_math_lang("math") && !is_math_lang("tex"));
    }
}
