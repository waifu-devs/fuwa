//! GPUI hands a scroll-wheel event to every scrolling box under the pointer,
//! not just the front one, unless something drawn in front of them
//! `.occlude()`s. Two ways that goes wrong, both of which happened:
//!
//! - A menu, picker or card floating over a page: scrolling it scrolls the
//!   page behind too. It needs `.occlude()` (or to sit over a layer that
//!   does, like `overlay::scrim`), and to be drawn after what it covers.
//! - A box that scrolls inside a page, a dialog or the chat: both move at
//!   once. It's made with `widgets::inner_scroll`, which keeps the wheel
//!   while it has more to show and lets it through otherwise.
//!
//! Neither can be told from the code alone, so this counts the places that
//! decide it, per file, and fails when one appears: look at the new one,
//! handle it as above, then update the count here.

use std::collections::BTreeMap;
use std::path::Path;

/// Boxes that scroll by themselves (`overflow_*_scroll`): a page, a panel,
/// a dialog's body, a popup's own list. Nothing scrolls behind them, or what
/// floats over that occludes. Anything inside something that scrolls goes
/// through `widgets::inner_scroll` instead.
const OUTER_SCROLLS: &[(&str, usize)] = &[
    ("attachments.rs", 1),
    ("connect.rs", 2),
    ("context_menu.rs", 2),
    ("dialogs.rs", 1),
    ("dm_dialogs.rs", 1),
    ("friends.rs", 1),
    ("gifs.rs", 2),
    ("instance_home.rs", 2),
    ("instance_settings.rs", 2),
    ("join.rs", 2),
    ("keys.rs", 2),
    ("onboarding.rs", 1),
    ("overlay.rs", 1),
    ("pins.rs", 1),
    ("polls.rs", 2),
    ("profile_card.rs", 2),
    ("rail.rs", 1),
    ("recordings.rs", 1),
    ("screen_share.rs", 1),
    ("search.rs", 2),
    ("secure_threads.rs", 2),
    ("server_settings.rs", 2),
    ("server_settings/menu.rs", 1),
    ("server_settings/overview.rs", 1),
    ("settings.rs", 2),
    ("settings_menu.rs", 1),
    ("sidebar.rs", 1),
    ("threads.rs", 2),
    ("voice_stage.rs", 1),
    ("widgets.rs", 1),
];

/// Things drawn over everything else (`deferred(`, `anchored()`): each one
/// that takes the mouse occludes, so the wheel and clicks stop at it.
const FLOATING: &[(&str, usize)] = &[
    ("announcement.rs", 1),
    ("attachments.rs", 2),
    ("call_parts.rs", 2),
    ("context_menu.rs", 1),
    ("emoji_picker.rs", 2),
    ("friends.rs", 2),
    ("menus.rs", 1),
    ("motion.rs", 1),
    ("pins.rs", 2),
    ("profile_card.rs", 3),
    ("rail.rs", 2),
    ("search.rs", 1),
    ("server_settings/menu.rs", 2),
    ("server_settings/overview.rs", 1),
    ("settings_menu.rs", 1),
    ("sidebar.rs", 2),
];

fn count(patterns: &[&str]) -> BTreeMap<String, usize> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    let mut found = BTreeMap::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let n: usize = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .map(|l| patterns.iter().map(|p| l.matches(p).count()).sum::<usize>())
                .sum();
            if n > 0 {
                let name = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                found.insert(name, n);
            }
        }
    }
    found
}

fn check(what: &str, patterns: &[&str], known: &[(&str, usize)], advice: &str) {
    let known: BTreeMap<String, usize> = known.iter().map(|(f, n)| (f.to_string(), *n)).collect();
    let found = count(patterns);
    let mut wrong = Vec::new();
    for file in known.keys().chain(found.keys()).collect::<std::collections::BTreeSet<_>>() {
        let (was, now) = (known.get(file).copied().unwrap_or(0), found.get(file).copied().unwrap_or(0));
        if was != now {
            wrong.push(format!("  src/ui/{file}: {was} known, {now} now"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{what} changed:\n{}\n\n{advice}\nThen update the count in desktop/tests/scrolling.rs.",
        wrong.join("\n")
    );
}

#[test]
fn every_scroll_box_is_decided() {
    check(
        "Scrolling boxes (overflow_*_scroll)",
        &["overflow_y_scroll()", "overflow_x_scroll()", "overflow_scroll()"],
        OUTER_SCROLLS,
        "A box that scrolls inside something else that scrolls (a page, a dialog, the chat) must be \
         `widgets::inner_scroll`, or the wheel scrolls both at once.",
    );
}

#[test]
fn every_floating_layer_is_decided() {
    check(
        "Floating layers (deferred, anchored)",
        &["deferred(", "anchored()"],
        FLOATING,
        "Anything drawn over a scrolling page that takes the mouse must `.occlude()` (or sit over a \
         layer that does), or scrolling it scrolls the page behind too.",
    );
}
