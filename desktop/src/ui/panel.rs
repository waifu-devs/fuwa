//! The window's parts as views of their own, so a frame of one part's
//! animation doesn't build the whole window again.
//!
//! GPUI draws the window's root view (`FuwaApp`) on every frame, whatever
//! asked for it, and a view asking for an animation frame marks itself and
//! every view around it as changed. Views drawn `cached` are kept as they were
//! drawn unless they were marked. So the root draws little itself: the
//! backdrop, the rail, the sidebar and the main column are cached panels,
//! drawn again only when `FuwaApp` says it changed (any `cx.notify()` on it,
//! which every panel follows) or when something inside them moves; the top
//! banners and everything over the window (dialogs, menus, toasts) are
//! panels too, not cached (their heights come from what's in them), only so
//! their animation frames mark them and not `FuwaApp`, which would mark
//! every panel.
//!
//! The parts themselves are still drawn by `FuwaApp` (`render_part`), with
//! its state, so listeners and `cx.notify()` inside them work as before.

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    StyleRefinement, Styled as _, Subscription, WeakEntity, Window, div, image_cache, px,
};

use crate::ui::app::FuwaApp;
use crate::ui::images::Images;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    /// What's drawn behind the app (`backdrop.rs`), with its moving effect.
    Backdrop,
    /// The banners across the top (streamer mode, the announcement, signing in again).
    Top,
    Rail,
    Sidebar,
    /// The open channel, conversation or page.
    Main,
    /// Small looping decorations placed in the cached parts (`floats.rs`).
    Floats,
    /// Everything over the window: dialogs, menus, sheets, toasts.
    Over,
}

pub struct Panel {
    app: WeakEntity<FuwaApp>,
    part: Part,
    /// The window's pictures, kept within a budget (`images.rs`), for a cached part.
    images: Option<Entity<Images>>,
    _follow: Option<Subscription>,
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let part = self.part;
        if self._follow.is_some() {
            crate::ui::floats::panel_drawn(cx.entity_id());
        }
        let el = self
            .app
            .update(cx, |app, cx| app.render_part(part, window, cx))
            .unwrap_or_else(|_| div().into_any_element());
        match &self.images {
            Some(images) => image_cache(images.clone()).size_full().child(el).into_any_element(),
            None => el,
        }
    }
}

/// Every part's view, made once with the window.
pub struct Panels {
    pub images: Entity<Images>,
    backdrop: Entity<Panel>,
    top: Entity<Panel>,
    rail: Entity<Panel>,
    sidebar: Entity<Panel>,
    main: Entity<Panel>,
    floats: Entity<Panel>,
    over: Entity<Panel>,
}

impl Panels {
    pub fn new(app: &Entity<FuwaApp>, cx: &mut App) -> Self {
        let images = Images::new(cx);
        let mut make = |part: Part, cached: bool| {
            let weak = app.downgrade();
            let kept = cached.then(|| images.clone());
            cx.new(|cx| Panel {
                app: weak,
                part,
                images: kept,
                // A cached part is drawn again whenever the app says something changed.
                _follow: cached.then(|| cx.observe(app, |_, _, cx| cx.notify())),
            })
        };
        Self {
            images: images.clone(),
            backdrop: make(Part::Backdrop, true),
            top: make(Part::Top, false),
            rail: make(Part::Rail, true),
            sidebar: make(Part::Sidebar, true),
            main: make(Part::Main, true),
            floats: make(Part::Floats, false),
            over: make(Part::Over, false),
        }
    }

    /// A part as an element of the root, sized as the root lays it out.
    pub fn element(&self, part: Part) -> AnyElement {
        let view = |e: &Entity<Panel>| AnyView::from(e.clone());
        match part {
            Part::Backdrop => view(&self.backdrop)
                .cached(StyleRefinement::default().absolute().top_0().left_0().size_full())
                .into_any_element(),
            Part::Rail => view(&self.rail)
                .cached(StyleRefinement::default().w(px(crate::ui::rail::RAIL)).h_full().flex_none())
                .into_any_element(),
            Part::Sidebar => view(&self.sidebar)
                .cached(StyleRefinement::default().w(px(crate::ui::sidebar::SIDEBAR)).h_full().flex_none())
                .into_any_element(),
            Part::Main => {
                view(&self.main).cached(StyleRefinement::default().flex_1().h_full().min_w_0()).into_any_element()
            }
            Part::Top => div().flex_none().child(self.top.clone()).into_any_element(),
            Part::Floats => self.floats.clone().into_any_element(),
            Part::Over => self.over.clone().into_any_element(),
        }
    }

    /// Draws one part again, and not the rest (a camera's new picture in the main column).
    pub fn notify(&self, part: Part, cx: &mut App) {
        let panel = match part {
            Part::Backdrop => &self.backdrop,
            Part::Top => &self.top,
            Part::Rail => &self.rail,
            Part::Sidebar => &self.sidebar,
            Part::Main => &self.main,
            Part::Floats => &self.floats,
            Part::Over => &self.over,
        };
        panel.update(cx, |_, cx| cx.notify());
    }

    /// The cached parts in the window's main layout (the backdrop too when `behind`).
    pub fn cached_ids(&self, behind: bool) -> Vec<gpui_kit::EntityId> {
        let mut ids = vec![self.rail.entity_id(), self.sidebar.entity_id(), self.main.entity_id()];
        if behind {
            ids.push(self.backdrop.entity_id());
        }
        ids
    }
}
