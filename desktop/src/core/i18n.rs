//! Translations: the same catalogs as the web app (`locales/` at the repo
//! root), built into the app by build.rs, so a new language is a new folder
//! there and nothing else (unless its plural rule is new, see `plural`).
//!
//! A value is text with `{name}` placeholders, or plural forms keyed by CLDR
//! category. Keys missing from a language show in English; a key missing from
//! English shows as itself, so a gap is visible rather than blank.

use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::RwLock;
use serde::Deserialize;

// (language, namespace, the file's text) for every catalog under locales/.
include!(concat!(env!("OUT_DIR"), "/locales.rs"));

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Entry {
    Text(String),
    Plural(HashMap<String, String>),
}

pub type Catalog = HashMap<String, Entry>;

/// A language's locales/<code>/meta.json.
#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    /// Its own name for itself ("Español").
    pub name: String,
    /// Its name in English ("Spanish").
    pub english: String,
    pub dir: String,
    /// The plural rule family (see `plural`).
    pub plural: String,
    /// False while it's a draft nobody who speaks it has checked.
    pub reviewed: bool,
    /// Digit grouping and decimal marks; English's when left out.
    #[serde(default = "comma")]
    pub group: String,
    #[serde(default = "dot")]
    pub decimal: String,
    /// The fewest digits before grouping starts: Spanish writes 1000 but 10.000.
    #[serde(default = "one", rename = "minGroup")]
    pub min_group: usize,
}

fn comma() -> String {
    ",".into()
}
fn dot() -> String {
    ".".into()
}
fn one() -> usize {
    1
}

#[derive(Debug, Clone)]
pub struct Language {
    pub code: String,
    pub meta: Meta,
}

/// Every language this build ships, English first.
pub static LANGUAGES: LazyLock<Vec<Language>> = LazyLock::new(|| {
    let mut out: Vec<Language> = FILES
        .iter()
        .filter(|(code, ns, _)| *ns == "meta" && is_tag(code))
        .filter_map(|(code, _, text)| Some(Language { code: code.to_string(), meta: serde_json::from_str(text).ok()? }))
        .collect();
    out.sort_by(|a, b| (a.code != "en").cmp(&(b.code != "en")).then_with(|| a.meta.english.cmp(&b.meta.english)));
    out
});

/// One language's catalogs, flattened to "namespace.key". A file that doesn't parse is left out (the tests catch it).
pub fn catalog(code: &str) -> Catalog {
    let mut out = Catalog::new();
    for (lang, ns, text) in FILES {
        if *lang != code || *ns == "meta" {
            continue;
        }
        if let Ok(entries) = serde_json::from_str::<HashMap<String, Entry>>(text) {
            out.extend(entries.into_iter().map(|(k, v)| (format!("{ns}.{k}"), v)));
        }
    }
    out
}

/// How much of English a language covers, from 0 to 1.
pub fn coverage(code: &str) -> f32 {
    let english = &ENGLISH;
    let theirs = catalog(code);
    if english.is_empty() {
        return 1.0;
    }
    english.keys().filter(|k| theirs.contains_key(*k)).count() as f32 / english.len() as f32
}

static ENGLISH: LazyLock<Catalog> = LazyLock::new(|| catalog("en"));

struct State {
    code: String,
    meta: Meta,
    catalog: Catalog,
}

static STATE: LazyLock<RwLock<State>> = LazyLock::new(|| RwLock::new(load("en")));

fn load(code: &str) -> State {
    let language = LANGUAGES.iter().find(|l| l.code == code).or_else(|| LANGUAGES.first()).expect("English ships");
    State { code: language.code.clone(), meta: language.meta.clone(), catalog: catalog(&language.code) }
}

/// Our tags: "en", "es", "pt-BR", "zh-Hant".
pub fn is_tag(value: &str) -> bool {
    let mut parts = value.split('-');
    let base = parts.next().unwrap_or_default();
    let base_ok = (2..=3).contains(&base.len()) && base.bytes().all(|b| b.is_ascii_lowercase());
    let rest_ok = match (parts.next(), parts.next()) {
        (None, _) => true,
        (Some(region), None) if region.len() == 2 => region.bytes().all(|b| b.is_ascii_uppercase()),
        (Some(script), None) if script.len() == 4 => {
            let b = script.as_bytes();
            b[0].is_ascii_uppercase() && b[1..].iter().all(|c| c.is_ascii_lowercase())
        }
        _ => false,
    };
    value.len() <= 12 && base_ok && rest_ok
}

/// The shipped language closest to what's asked for: exact, then the same language ("es-MX" gets "es"), else English.
pub fn negotiate<S: AsRef<str>>(requested: &[S], available: &[&str]) -> String {
    for want in requested {
        let want = want.as_ref().trim().replace('_', "-").to_ascii_lowercase();
        if want.is_empty() || want == "*" {
            continue;
        }
        if let Some(code) = available.iter().find(|c| c.to_ascii_lowercase() == want) {
            return code.to_string();
        }
        let base = want.split('-').next().unwrap_or_default();
        if let Some(code) = available.iter().find(|c| c.to_ascii_lowercase() == base) {
            return code.to_string();
        }
    }
    "en".into()
}

/// The languages the system is set to, most wanted first. Read only to pick one; never stored or sent.
/// What the computer asks for, read once: asking runs a program on macOS and
/// Windows, which shouldn't happen again on every settings change.
pub fn system_languages() -> Vec<String> {
    static ONCE: LazyLock<Vec<String>> = LazyLock::new(read_system_languages);
    ONCE.clone()
}

fn read_system_languages() -> Vec<String> {
    let mut out = Vec::new();
    // Linux and the BSDs, and anyone who sets them elsewhere.
    for var in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(var) {
            for item in value.split(':') {
                let tag = item.split(['.', '@']).next().unwrap_or_default();
                if !tag.is_empty() && tag != "C" && tag != "POSIX" {
                    out.push(tag.to_string());
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(text) =
        run_bounded(std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLanguages"]))
    {
        // ( "es-MX", "en-US" )
        out.splice(0..0, text.split('"').skip(1).step_by(2).take(16).map(str::to_string).collect::<Vec<_>>());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let reg = std::path::Path::new(&root).join("System32").join("reg.exe");
        let mut command = std::process::Command::new(reg);
        // CREATE_NO_WINDOW: no console flashes up.
        command.args(["query", r"HKCU\Control Panel\International", "/v", "LocaleName"]).creation_flags(0x0800_0000);
        if let Some(text) = run_bounded(&mut command) {
            // "    LocaleName    REG_SZ    es-MX"
            let value = text.lines().find_map(|l| l.split_once("REG_SZ").map(|(_, v)| v.trim().to_string()));
            if let Some(tag) = value.filter(|v| !v.is_empty()) {
                out.insert(0, tag);
            }
        }
    }
    out.truncate(24);
    out
}

/// Runs a system tool for its answer: no input, no error output, at most 4 KiB
/// read, and killed after 2 seconds. Its output is never logged.
#[cfg(any(target_os = "macos", windows))]
fn run_bounded(command: &mut std::process::Command) -> Option<String> {
    use std::io::Read as _;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = command.stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped()).spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    child.stdout.take()?.take(4096).read_to_string(&mut text).ok()?;
    Some(text)
}

/// The shipped language the system's languages match best.
pub fn system_language() -> String {
    let codes: Vec<&str> = LANGUAGES.iter().map(|l| l.code.as_str()).collect();
    negotiate(&system_languages(), &codes)
}

/// Switches the app's language: a shipped code, or None to follow the system.
/// The caller refreshes the windows.
pub fn set_language(setting: Option<&str>) {
    let code = match setting {
        Some(code) if LANGUAGES.iter().any(|l| l.code == code) => code.to_string(),
        _ => system_language(),
    };
    if STATE.read().code != code {
        *STATE.write() = load(&code);
    }
}

/// The language the app is in, and its meta.json.
pub fn current() -> (String, Meta) {
    let state = STATE.read();
    (state.code.clone(), state.meta.clone())
}

/// A value for a placeholder: text, or a number shown in the language's digits (and picking plurals as `count`).
pub enum Arg<'a> {
    Str(&'a str),
    Num(i64),
}

/// A string in the app's language.
pub fn t(key: &str) -> String {
    t_with(key, &[])
}

/// A string with its placeholders filled; `count` picks the plural form.
pub fn t_with(key: &str, args: &[(&str, Arg)]) -> String {
    let state = STATE.read();
    let count = args.iter().find_map(|(name, arg)| match (name, arg) {
        (&"count", Arg::Num(n)) => Some(*n),
        _ => None,
    });
    let text = template(&state.meta.plural, &state.catalog, &ENGLISH, key, count);
    fill(&text, |name| {
        args.iter().find(|(n, _)| *n == name).map(|(_, arg)| match arg {
            Arg::Str(s) => s.to_string(),
            Arg::Num(n) => format_number(*n, &state.meta),
        })
    })
}

/// The template for a key: the language's, else English's (with English plural rules), else the key.
pub fn template(rule: &str, catalog: &Catalog, english: &Catalog, key: &str, count: Option<i64>) -> String {
    let (entry, rule) = match catalog.get(key) {
        Some(entry) => (entry, rule),
        None => match english.get(key) {
            Some(entry) => (entry, "one-other"),
            None => return key.to_string(),
        },
    };
    match entry {
        Entry::Text(text) => text.clone(),
        Entry::Plural(forms) => {
            let category = plural(rule, count.unwrap_or(0).unsigned_abs());
            forms.get(category).or_else(|| forms.get("other")).cloned().unwrap_or_default()
        }
    }
}

/// Fills `{name}` placeholders; unknown ones stay as written, so they show up in review.
pub fn fill(text: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let name_len = after.find('}').filter(|&end| {
            let name = &after[..end];
            name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && name.chars().all(|c| c.is_ascii_alphanumeric())
        });
        match name_len.and_then(|end| value(&after[..end]).map(|v| (end, v))) {
            Some((end, v)) => {
                out.push_str(&v);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The CLDR plural category of a whole number, by the rule family a meta.json names.
/// A language whose rule isn't here needs one arm (and the same name in the tests' list).
pub fn plural(rule: &str, n: u64) -> &'static str {
    let (n10, n100) = (n % 10, n % 100);
    match rule {
        "one-other" => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
        // Spanish, French, Italian, Portuguese: exact millions take "many" ("1 millón de").
        "one-many-other-romance" => {
            if n == 1 {
                "one"
            } else if n != 0 && n.is_multiple_of(1_000_000) {
                "many"
            } else {
                "other"
            }
        }
        // Russian, Ukrainian.
        "one-few-many-other-slavic" => {
            if n10 == 1 && n100 != 11 {
                "one"
            } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
                "few"
            } else {
                "many"
            }
        }
        "one-few-many-other-polish" => {
            if n == 1 {
                "one"
            } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
                "few"
            } else {
                "many"
            }
        }
        "one-few-other-romanian" => {
            if n == 1 {
                "one"
            } else if n == 0 || (1..=19).contains(&n100) {
                "few"
            } else {
                "other"
            }
        }
        "zero-one-other-latvian" => {
            if n10 == 0 || (11..=19).contains(&n100) {
                "zero"
            } else if n10 == 1 && n100 != 11 {
                "one"
            } else {
                "other"
            }
        }
        "one-two-other" => match n {
            1 => "one",
            2 => "two",
            _ => "other",
        },
        "one-two-few-many-other-arabic" => match (n, n100) {
            (0, _) => "zero",
            (1, _) => "one",
            (2, _) => "two",
            (_, 3..=10) => "few",
            (_, 11..=99) => "many",
            _ => "other",
        },
        _ => "other",
    }
}

/// A whole number written as the app's language writes it ("1,234" in English).
pub fn number(n: i64) -> String {
    format_number(n, &STATE.read().meta)
}

/// A whole number with the language's digit grouping: "1,234" in English, "1234" and "12.345" in Spanish.
pub fn format_number(n: i64, meta: &Meta) -> String {
    let digits = n.unsigned_abs().to_string();
    let sign = if n < 0 { "-" } else { "" };
    if digits.len() < 4 + meta.min_group - 1 {
        return format!("{sign}{digits}");
    }
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(&meta.group);
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(group: &str, min_group: usize) -> Meta {
        Meta {
            name: String::new(),
            english: String::new(),
            dir: "ltr".into(),
            plural: "one-other".into(),
            reviewed: true,
            group: group.into(),
            decimal: ".".into(),
            min_group,
        }
    }

    #[test]
    fn fills_placeholders_and_leaves_unknown_ones() {
        let v = |name: &str| (name == "server").then(|| "Cozy".to_string());
        assert_eq!(fill("Joined {server}", v), "Joined Cozy");
        assert_eq!(fill("Hi {name}", v), "Hi {name}");
        assert_eq!(fill("{Not} a {place-holder} {", v), "{Not} a {place-holder} {");
        // One pass: a value that looks like a placeholder goes in as written.
        let sneaky = |name: &str| (name == "server").then(|| "{server}".to_string());
        assert_eq!(fill("Joined {server}", sneaky), "Joined {server}");
    }

    #[test]
    fn plural_families_match_cldr() {
        assert_eq!(plural("one-other", 1), "one");
        assert_eq!(plural("one-other", 0), "other");
        assert_eq!(plural("one-many-other-romance", 1), "one");
        assert_eq!(plural("one-many-other-romance", 1_000_000), "many");
        assert_eq!(plural("one-many-other-romance", 1_000_001), "other");
        assert_eq!(plural("one-few-many-other-slavic", 21), "one");
        assert_eq!(plural("one-few-many-other-slavic", 22), "few");
        assert_eq!(plural("one-few-many-other-slavic", 12), "many");
        assert_eq!(plural("one-few-many-other-polish", 22), "few");
        assert_eq!(plural("one-few-many-other-polish", 21), "many");
        assert_eq!(plural("one-two-few-many-other-arabic", 105), "few");
    }

    #[test]
    fn looks_up_language_then_english_then_key() {
        let en: Catalog = [
            ("c.save".to_string(), Entry::Text("Save".into())),
            (
                "c.files".to_string(),
                Entry::Plural([("one".into(), "{count} file".into()), ("other".into(), "{count} files".into())].into()),
            ),
        ]
        .into();
        let es: Catalog = [("c.save".to_string(), Entry::Text("Guardar".into()))].into();
        assert_eq!(template("one-many-other-romance", &es, &en, "c.save", None), "Guardar");
        assert_eq!(template("one-many-other-romance", &es, &en, "c.files", Some(1)), "{count} file");
        assert_eq!(template("one-many-other-romance", &es, &en, "c.nope", None), "c.nope");
    }

    #[test]
    fn negotiates_and_checks_tags() {
        let shipped = ["en", "es", "pt-BR"];
        assert_eq!(negotiate(&["es-MX", "en"], &shipped), "es");
        assert_eq!(negotiate(&["pt_br"], &shipped), "pt-BR");
        assert_eq!(negotiate(&["fr"], &shipped), "en");
        for ok in ["en", "es", "pt-BR", "zh-Hant", "fil"] {
            assert!(is_tag(ok), "{ok}");
        }
        for bad in ["", "EN", "en-", "../en", "en-us", "es\n"] {
            assert!(!is_tag(bad), "{bad:?}");
        }
    }

    #[test]
    fn groups_digits_like_the_language() {
        assert_eq!(format_number(1234, &meta(",", 1)), "1,234");
        assert_eq!(format_number(-1234567, &meta(",", 1)), "-1,234,567");
        assert_eq!(format_number(1234, &meta(".", 2)), "1234");
        assert_eq!(format_number(12345, &meta(".", 2)), "12.345");
        assert_eq!(format_number(999, &meta(",", 1)), "999");
    }

    #[test]
    fn every_catalog_parses_and_matches_english() {
        assert!(LANGUAGES.iter().any(|l| l.code == "en"), "English ships");
        for (code, ns, text) in FILES {
            assert!(is_tag(code), "locales/{code} isn't a tag");
            if *ns == "meta" {
                let meta: Meta = serde_json::from_str(text).unwrap_or_else(|e| panic!("locales/{code}/meta.json: {e}"));
                assert_ne!(plural(&meta.plural, 1), "", "{code}");
                continue;
            }
            let entries: HashMap<String, Entry> =
                serde_json::from_str(text).unwrap_or_else(|e| panic!("locales/{code}/{ns}.json: {e}"));
            for key in entries.keys() {
                assert!(ENGLISH.contains_key(&format!("{ns}.{key}")), "locales/{code}/{ns}.json {key}: not in English");
            }
        }
    }

    /// Every `.rs` file under src/ but this one, with its text.
    fn sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n.rs") {
                    out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    /// Every `t("key")` and `t_with("key", ...)` in a file, as (function, key, the text after the key).
    fn literal_calls(text: &str) -> Vec<(&'static str, &str, &str)> {
        let mut out = Vec::new();
        for name in ["t", "t_with"] {
            for (at, _) in text.match_indices(&format!("{name}(")) {
                // Our functions, not `expect("…")` or a method of the same name.
                let before = text[..at].chars().next_back().unwrap_or(' ');
                if before.is_alphanumeric() || before == '_' || before == '.' {
                    continue;
                }
                // The key may sit on the next line once rustfmt wraps the call.
                let Some(rest) = text[at + name.len() + 1..].trim_start().strip_prefix('"') else { continue };
                if let Some(end) = rest.find('"') {
                    out.push((name, &rest[..end], &rest[end + 1..]));
                }
            }
        }
        out
    }

    fn shaped(key: &str) -> bool {
        key.split('.').count() >= 2
            && key.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()))
    }

    /// The `{name}` placeholders `fill` would fill in a template.
    fn placeholders(text: &str) -> std::collections::BTreeSet<String> {
        let out = std::cell::RefCell::new(std::collections::BTreeSet::new());
        fill(text, |name| {
            out.borrow_mut().insert(name.to_string());
            None
        });
        out.into_inner()
    }

    /// Every key the app asks for exists in English.
    #[test]
    fn keys_in_the_code_exist() {
        for (file, text) in sources() {
            for (_, key, _) in literal_calls(&text) {
                if shaped(key) {
                    assert!(ENGLISH.contains_key(key), "{file}: \"{key}\" isn't in locales/en");
                }
            }
        }
    }

    /// Each call fills exactly the placeholders its English has: `t` only for
    /// keys without any, `t_with` naming every one (and `count` for plurals).
    #[test]
    fn calls_fill_their_placeholders() {
        let mut problems = Vec::new();
        for (file, text) in sources() {
            for (call, key, rest) in literal_calls(&text) {
                let Some(entry) = ENGLISH.get(key) else { continue };
                let (mut wanted, plural) = match entry {
                    Entry::Text(text) => (placeholders(text), false),
                    Entry::Plural(forms) => (forms.values().flat_map(|f| placeholders(f)).collect(), true),
                };
                if plural {
                    wanted.insert("count".to_string());
                }
                if call == "t" {
                    if !wanted.is_empty() {
                        problems.push(format!("{file}: t(\"{key}\") has {wanted:?} to fill; use t_with"));
                    }
                    continue;
                }
                // The args: `, &[ ... ]` right after the key, up to the matching `]`.
                let Some(list) = rest.trim_start().strip_prefix(',').map(str::trim_start) else {
                    problems.push(format!("{file}: t_with(\"{key}\" has no args"));
                    continue;
                };
                let Some(list) = list.strip_prefix("&[") else {
                    problems.push(format!("{file}: t_with(\"{key}\", ...) needs its args written in place, as &[...]"));
                    continue;
                };
                let mut depth = 1;
                let end = list
                    .char_indices()
                    .find(|&(_, c)| {
                        match c {
                            '[' => depth += 1,
                            ']' => depth -= 1,
                            _ => {}
                        }
                        depth == 0
                    })
                    .map_or(list.len(), |(i, _)| i);
                let list = &list[..end];
                // Each `("name"` that starts an arg (after `[` or `,`), not a call like `format!("…")`.
                // rustfmt may break the line between the `(` and the name.
                let given: std::collections::BTreeSet<String> = list
                    .match_indices('(')
                    .filter(|(at, _)| {
                        let before = list[..*at].trim_end();
                        before.is_empty() || before.ends_with(',')
                    })
                    .filter_map(|(at, _)| {
                        let name = list[at + 1..].trim_start().strip_prefix('"')?;
                        name.find('"').map(|end| name[..end].to_string())
                    })
                    .collect();
                if given != wanted {
                    problems.push(format!("{file}: t_with(\"{key}\") fills {given:?}, its English has {wanted:?}"));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}
