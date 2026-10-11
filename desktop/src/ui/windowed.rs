//! Long lists inside a page that scrolls as a whole (a server's members in
//! its settings, the Friends page, Browse's cards, the conversations in the
//! sidebar): only the rows near what's in sight are built, with empty space
//! standing in for the rest, so a list of thousands costs a screenful.
//!
//! Rows may be any height: each one's is measured as it's drawn and kept by
//! its key, and rows not drawn yet are guessed. The page's scroll box hands
//! over its `ScrollHandle` (`track_scroll`), which says where the page is
//! scrolled and how tall its window is; the list notes where it sits in the
//! page as it's drawn.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, ScrollHandle, SharedString, Styled as _, Window, div, px,
};

/// How far past what's in sight rows are built anyway, so a quick scroll doesn't show empty space.
const OVERSCAN: f32 = 600.0;
/// With nothing measured yet (the first frame), how much of the list is built.
const FIRST: f32 = 1600.0;

#[derive(Default)]
struct State {
    /// Each row's height as last drawn (with its gap).
    heights: HashMap<SharedString, f32>,
    /// Where the list starts in the page (from the top of what scrolls), once drawn.
    top: Option<f32>,
}

thread_local! {
    static LISTS: RefCell<HashMap<SharedString, Rc<RefCell<State>>>> = RefCell::new(HashMap::new());
}

/// Which rows of a list to build: [`Plan::rows`], then [`Plan::finish`] with them.
pub struct Plan {
    state: Rc<RefCell<State>>,
    shown: Vec<(usize, SharedString)>,
    above: f32,
    below: f32,
    last: usize,
    gap: f32,
    scroll: ScrollHandle,
}

/// A list of rows (`keys` name them) inside the page `scroll` tracks, `gap`
/// apart, each guessed `guess` tall until it's drawn. `id` names the list,
/// so its measurements are kept between frames.
pub fn plan(id: impl Into<SharedString>, scroll: &ScrollHandle, keys: &[SharedString], gap: f32, guess: f32) -> Plan {
    let id = id.into();
    let state = LISTS.with_borrow_mut(|lists| {
        // Lists not drawn for a while are forgotten with the rest, rather than one by one.
        if lists.len() > 32 {
            lists.clear();
        }
        lists.entry(id).or_default().clone()
    });
    let last = keys.len().saturating_sub(1);
    // What's in sight, from the top of the list.
    let (from, to) = {
        let s = state.borrow();
        let view = scroll.bounds();
        match s.top {
            Some(top) if view.size.height > px(0.0) => {
                let scrolled = -f32::from(scroll.offset().y);
                let from = scrolled - top - OVERSCAN;
                (from, from + f32::from(view.size.height) + 2.0 * OVERSCAN)
            }
            _ => (0.0, FIRST),
        }
    };
    let (mut above, mut below) = (0.0f32, 0.0f32);
    let mut shown: Vec<(usize, SharedString)> = Vec::new();
    {
        let s = state.borrow();
        let mut y = 0.0f32;
        for (n, key) in keys.iter().enumerate() {
            let h = s.heights.get(key).copied().unwrap_or(guess + if n == last { 0.0 } else { gap });
            if y + h < from {
                above += h;
            } else if y > to {
                below += h;
            } else {
                shown.push((n, key.clone()));
            }
            y += h;
        }
    }
    Plan { state, shown, above, below, last, gap, scroll: scroll.clone() }
}

impl Plan {
    /// The rows to build, by their place in the list.
    pub fn rows(&self) -> Vec<usize> {
        self.shown.iter().map(|(n, _)| *n).collect()
    }

    /// The list, from the rows built (in the order [`Plan::rows`] gave).
    pub fn finish(self, rows: Vec<AnyElement>, window: &Window) -> AnyElement {
        let Plan { state, shown, above, below, last, gap, scroll } = self;
        let mut list = div().flex().flex_col().child(div().h(px(above)));
        for ((n, _), row) in shown.iter().zip(rows) {
            list = list.child(div().when_gap(*n != last, gap).child(row));
        }
        list = list.child(div().h(px(below)));
        let view = window.current_view();
        list.on_children_prepainted(move |bounds, _, cx: &mut App| {
            let mut s = state.borrow_mut();
            let mut moved = false;
            if let Some(first) = bounds.first() {
                let page = scroll.bounds();
                let top = f32::from(first.origin.y - page.origin.y - scroll.offset().y);
                moved |= s.top.is_none_or(|t| (t - top).abs() > 0.5);
                s.top = Some(top);
            }
            for ((_, key), b) in shown.iter().zip(bounds.iter().skip(1)) {
                let h = f32::from(b.size.height);
                let was = s.heights.insert(key.clone(), h);
                moved |= was.is_none_or(|w| (w - h).abs() > 0.5);
            }
            if moved {
                // Drawn again with what was learned: the rows really in sight, and space that fits.
                cx.notify(view);
            }
        })
        .into_any_element()
    }
}

trait Gap {
    fn when_gap(self, on: bool, gap: f32) -> Self;
}

impl Gap for gpui_kit::Div {
    fn when_gap(self, on: bool, gap: f32) -> Self {
        if on { self.pb(px(gap)) } else { self }
    }
}
