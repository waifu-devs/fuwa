//! The "Profile items" page, in a server's settings (Manage Server) and in
//! an instance's (admins): the effects and avatar decorations it offers
//! (docs/profile-items.md). Add a decoration from a picture, an effect from
//! a `.json` file (sent as is; the instance checks it and says what's wrong),
//! rename or describe one in place, or delete it. One view for both, like
//! the web's `settings/ProfileItems.tsx`.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    Subscription, Window, div, img, px,
};

use crate::core::Core;
use crate::core::account::picture_type;
use crate::core::api::Problem;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::profile_items::{NewItem, Scope};
use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{danger_button, error_line, icon, icon_button, labeled, pal, primary_button, soft_button};

/// The most an effect's file may be, read before it's sent (the instance takes 16 KB).
const MOST_EFFECT: u64 = 64 * 1024;

pub struct ProfileItemsView {
    core: Arc<Core>,
    key: String,
    scope: Scope,
    /// The name and description a new item gets (its file's name when left empty).
    name: Entity<InputState>,
    description: Entity<InputState>,
    /// The item being renamed, and the box its name is typed in.
    renaming: Option<String>,
    rename: Entity<InputState>,
    /// The item asked about before it's deleted.
    confirming: Option<String>,
    busy: bool,
    /// An item was added: empty the name and description when next drawn.
    clear: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ProfileItemsView {
    pub fn new(core: Arc<Core>, key: String, scope: Scope, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.profileItems.namePlaceholder")));
        let description = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("serversettings.profileItems.descriptionPlaceholder")));
        let rename = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![cx.subscribe(&rename, |this: &mut Self, _, e: &InputEvent, cx| match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(cx),
            InputEvent::Change => cx.notify(),
            InputEvent::Focus => {}
        })];
        // The list lives in the store: draw again when it changes.
        let mut changes = core.changes();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            core,
            key,
            scope,
            name,
            description,
            renaming: None,
            rename,
            confirming: None,
            busy: false,
            clear: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// Runs a call on the core's runtime and hands its result back to the page.
    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        future: impl Future<Output = Result<T, Problem>> + Send + 'static,
        done: impl FnOnce(&mut Self, Result<T, Problem>, &mut Context<Self>) + 'static,
    ) {
        let rx = self.core.spawn(future);
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| done(this, result, cx));
        })
        .detach();
    }

    fn pick(&mut self, kind: pb::ProfileItemKind, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let prompt = match kind {
            pb::ProfileItemKind::Effect => t("desktop.profileItems.chooseEffect"),
            _ => t("desktop.account.choosePicture"),
        };
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| this.add(kind, path, cx));
        })
        .detach();
    }

    /// Reads the picked file and adds it as an item.
    fn add(&mut self, kind: pb::ProfileItemKind, path: PathBuf, cx: &mut Context<Self>) {
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let picture = picture_type(&file);
        let is_json = file.to_ascii_lowercase().ends_with(".json");
        match kind {
            pb::ProfileItemKind::Effect if !is_json => {
                self.error = Some(t("desktop.profileItems.notJson"));
                return cx.notify();
            }
            pb::ProfileItemKind::Decoration if picture.is_none() => {
                self.error = Some(t("serversettings.profileItems.badType"));
                return cx.notify();
            }
            _ => {}
        }
        let typed = self.name.read(cx).value().trim().to_owned();
        let name: String = if typed.is_empty() { name_from_file(&file) } else { typed };
        let description = self.description.read(cx).value().trim().to_owned();
        self.busy = true;
        self.error = None;
        cx.notify();
        let (core, key, scope) = (self.core.clone(), self.key.clone(), self.scope.clone());
        self.run(
            cx,
            async move {
                let new = match picture {
                    Some(content_type) if kind == pb::ProfileItemKind::Decoration => NewItem::Decoration {
                        content_type: content_type.to_owned(),
                        bytes: crate::core::account::read_picture(&path).await?,
                    },
                    _ => NewItem::Effect { spec: read_effect(&path).await? },
                };
                core.add_profile_item(&key, &scope, &name, &description, new).await
            },
            |this, result, cx| {
                this.busy = false;
                match result {
                    Ok(_) => this.clear = true,
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
    }

    fn start_rename(&mut self, item: &pb::ProfileItem, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = Some(item.id.clone());
        let name = item.name.clone();
        self.rename.update(cx, |s, cx| {
            s.set_value(name, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.renaming.take() else { return };
        let name = self.rename.read(cx).value().trim().to_owned();
        let unchanged = self.core.shared.read(|s| {
            s.instance(&self.key).is_some_and(|i| match &self.scope {
                Scope::Instance => i.instance_item(&id).is_some_and(|x| x.name == name),
                Scope::Server(sid) => i.server_item(sid, &id).is_some_and(|x| x.name == name),
            })
        });
        cx.notify();
        if name.is_empty() || unchanged {
            return;
        }
        let (core, key, scope) = (self.core.clone(), self.key.clone(), self.scope.clone());
        let change = pb::ProfileItemChange { name: Some(name), ..Default::default() };
        self.run(cx, async move { core.change_profile_item(&key, &scope, &id, change).await }, |this, result, cx| {
            if let Err(err) = result {
                this.error = Some(err.message);
            }
            cx.notify();
        });
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.confirming = None;
        self.busy = true;
        let (core, key, scope) = (self.core.clone(), self.key.clone(), self.scope.clone());
        self.run(cx, async move { core.delete_profile_item(&key, &scope, &id).await }, |this, result, cx| {
            this.busy = false;
            if let Err(err) = result {
                this.error = Some(err.message);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn row(&self, item: &pb::ProfileItem, n: usize, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id.clone();
        let decoration = item.kind == pb::ProfileItemKind::Decoration as i32;
        let face: AnyElement = if decoration && !item.picture_url.is_empty() {
            img(SharedString::from(item.picture_url.clone()))
                .size(px(44.0))
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else {
            div()
                .size(px(44.0))
                .rounded_full()
                .bg(alpha(p.primary, 0.12))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(if decoration { "circle-dashed" } else { "sparkles" }).size(px(20.0)).text_color(p.primary))
                .into_any_element()
        };
        let renaming = self.renaming.as_deref() == Some(item.id.as_str());
        let title: AnyElement = if renaming {
            div().w(px(240.0)).child(Input::new(&self.rename).small()).into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().font_weight(FontWeight::BOLD).child(item.name.clone()))
                .child(div().px(px(8.0)).rounded_full().bg(p.secondary).text_xs().text_color(p.muted_foreground).child(
                    if decoration { t("desktop.profileItems.decoration") } else { t("desktop.profileItems.effect") },
                ))
                .into_any_element()
        };
        let confirming = self.confirming.as_deref() == Some(item.id.as_str());
        let actions: AnyElement = if confirming {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(soft_button(SharedString::from(format!("pi-keep|{id}")), t("common.cancel"), p).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.confirming = None;
                        cx.notify();
                    }),
                ))
                .child(
                    danger_button(
                        SharedString::from(format!("pi-delete-yes|{id}")),
                        t("desktop.profileItems.delete"),
                        p,
                    )
                    .on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| this.delete(id.clone(), cx))
                    }),
                )
                .into_any_element()
        } else {
            let item = item.clone();
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .opacity(0.0)
                .group_hover("profile-item", |s| s.opacity(1.0))
                .child(
                    icon_button(SharedString::from(format!("pi-rename|{id}")), "pencil", p)
                        .on_click(cx.listener(move |this, _, window, cx| this.start_rename(&item, window, cx))),
                )
                .child(icon_button(SharedString::from(format!("pi-delete|{id}")), "trash", p).on_click({
                    let id = id.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.confirming = Some(id.clone());
                        cx.notify();
                    })
                }))
                .into_any_element()
        };
        motion::rise(
            div()
                .id(SharedString::from(format!("pi|{id}")))
                .group("profile-item")
                .flex()
                .items_center()
                .gap(px(14.0))
                .px(px(14.0))
                .py(px(10.0))
                .rounded(corner(14.0))
                .border_1()
                .border_color(if confirming { p.destructive } else { p.border })
                .bg(p.card)
                .hover(|s| s.bg(alpha(p.primary, 0.05)))
                .child(face)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(title)
                        .when(!item.description.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .text_ellipsis()
                                    .child(item.description.clone()),
                            )
                        })
                        .when(confirming, |el| {
                            el.child(
                                div()
                                    .text_sm()
                                    .text_color(p.destructive)
                                    .child(t_with("desktop.profileItems.deleteAsk", &[("name", Arg::Str(&item.name))])),
                            )
                        }),
                )
                .child(actions),
            SharedString::from(format!("pi-in|{}", item.id)),
            std::time::Duration::from_millis(30 * n.min(10) as u64),
            6.0,
        )
        .into_any_element()
    }
}

/// "Cherry Blossom.json" → "Cherry Blossom", at most 40 characters.
fn name_from_file(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let name: String = stem.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().take(40).collect();
    if name.is_empty() { "New item".to_owned() } else { name }
}

/// An effect's spec from its file, as text: the instance checks the rest.
async fn read_effect(path: &std::path::Path) -> Result<String, Problem> {
    let unreadable = || Problem::new(tonic::Code::NotFound, t("desktop.look.cantRead"));
    let size = tokio::fs::metadata(path).await.map_err(|_| unreadable())?.len();
    if size > MOST_EFFECT {
        return Err(Problem::new(tonic::Code::InvalidArgument, t("desktop.profileItems.effectTooBig")));
    }
    let text = tokio::fs::read_to_string(path).await.map_err(|_| unreadable())?;
    if serde_json::from_str::<serde_json::Value>(&text).is_err() {
        return Err(Problem::new(tonic::Code::InvalidArgument, t("desktop.profileItems.notJson")));
    }
    Ok(text)
}

impl Render for ProfileItemsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        if std::mem::take(&mut self.clear) {
            self.name.update(cx, |s, cx| s.set_value("", window, cx));
            self.description.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let (items, on) = self.core.shared.read(|s| match s.instance(&self.key) {
            Some(i) => {
                let items = match &self.scope {
                    Scope::Instance => i.profile_items.clone(),
                    Scope::Server(sid) => i.server_items.get(sid).cloned().unwrap_or_default(),
                };
                (items, i.decorations_on())
            }
            None => (Vec::new(), false),
        });
        let adding = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(corner(16.0))
            .bg(p.secondary)
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .child(div().w(px(220.0)).child(labeled(
                        &t("serversettings.profileItems.name"),
                        Input::new(&self.name),
                        &p,
                    )))
                    .child(div().flex_1().child(labeled(
                        &t("serversettings.profileItems.description"),
                        Input::new(&self.description),
                        &p,
                    ))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        primary_button(
                            "pi-add-decoration",
                            if self.busy {
                                t("serversettings.emoji.uploading")
                            } else {
                                t("serversettings.profileItems.addDecoration")
                            },
                            &p,
                        )
                        .h(px(36.0))
                        .text_sm()
                        .child(icon("image-up").size(px(16.0)))
                        .on_click(cx.listener(|this, _, _, cx| this.pick(pb::ProfileItemKind::Decoration, cx))),
                    )
                    .child(
                        soft_button("pi-add-effect", t("serversettings.profileItems.addEffect"), &p)
                            .child(icon("file-braces").size(px(16.0)))
                            .on_click(cx.listener(|this, _, _, cx| this.pick(pb::ProfileItemKind::Effect, cx))),
                    ),
            )
            .child(div().text_xs().text_color(p.muted_foreground).child(t("desktop.profileItems.addHint")));
        let mut list = div().flex().flex_col().gap(px(8.0));
        if items.is_empty() {
            list = list.child(
                div()
                    .py(px(28.0))
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("desktop.profileItems.none")),
            );
        }
        for (n, item) in items.iter().enumerate() {
            list = list.child(self.row(item, n, &p, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .when(!on, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(icon("eye-off").size(px(14.0)))
                        .child(t("desktop.profileItems.decorationsOff")),
                )
            })
            .child(adding)
            .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(e))
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_files() {
        assert_eq!(name_from_file("Cherry_Blossom.json"), "Cherry Blossom");
        assert_eq!(name_from_file("ring-gold.png"), "ring gold");
        assert_eq!(name_from_file(".json"), "New item");
    }
}
