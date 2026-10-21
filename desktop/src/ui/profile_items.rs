//! The "Profile items" page, in a server's settings (Manage Server) and in
//! an instance's (admins): the effects, avatar decorations and nameplates it
//! offers (docs/profile-items.md), as the web's `settings/ProfileItems.tsx`.
//! Decorations and nameplates are pictures dropped in or picked, shown around
//! your avatar or behind your row before they're added; effects are specs, pasted or from a `.json` file,
//! checked and played before they're added, with the pictures their layers
//! draw and a shader under them where the instance has those (the WGSL
//! edited apart from the JSON). Each one's name and line change
//! in place, an effect can take a new spec, and deleting one asks first,
//! since it takes it off everyone wearing it. Under each kind are the
//! helpers for making one: a template to draw a decoration or nameplate on, prompts to
//! copy for an AI and the guides; a new effect can start from a built-in.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, AppContext as _, ClipboardItem, Context, Entity, ExternalPaths, FontWeight,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Window, div, img, px, rgb,
};

use crate::core::Core;
use crate::core::account::picture_type;
use crate::core::api::Problem;
use crate::core::effects::custom::{STARTERS, shader_id, shader_problem};
use crate::core::effects::gpu::{self, Gpu};
use crate::core::effects::status::{self, ShaderStatus};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::profile_effects::{self as fx, Spec};
use crate::core::profile_items::{
    DECORATION_DOCS, DECORATION_PROMPT, DraftPicture, EFFECT_DOCS, EFFECT_PROMPT, NAMEPLATE_DOCS, NAMEPLATE_HEIGHT,
    NAMEPLATE_PROMPT, NAMEPLATE_WIDTH, NewItem, Scope, TEMPLATE_SIZE, check_effect_text, item_effect, join_shader,
    name_from_file, nameplate_template_pixels, split_shader, starter_text, template_pixels,
};
use crate::pb;
use crate::ui::motion;
use crate::ui::profile_effect::{EffectView, effect_in, mini_card};
use crate::ui::settings_controls::{Look, button, shadow_sm};
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_xl};
use crate::ui::widgets::{avatar, decorated, icon, pal};

/// The most an effect's file may be, read before it's put in the box (the instance takes 16 KB).
const MOST_EFFECT: u64 = 64 * 1024;
const NAME_MAX: usize = 40;
const DESCRIPTION_MAX: usize = 120;
/// How long the shader's code rests before the preview compiles it.
const SHADER_REST: Duration = Duration::from_millis(450);
/// The instance feature that lets effects carry pictures and a shader.
const MEDIA: &str = "profile-effect-media";

/// An effect being written, beside its JSON box: its shader's WGSL apart
/// from the JSON, and its pictures (the web's `useEffectDraft`).
struct EffectDraft {
    wgsl: Entity<TextareaState>,
    pictures: Vec<DraftPicture>,
    /// The shader's field is open: someone added one, or there is one.
    shader_open: bool,
    /// The code the preview plays: the box's, once typing has rested.
    resting: String,
    typed: u64,
    /// What it started from (an effect being changed), to tell whether anything changed.
    initial: String,
    initial_urls: Vec<String>,
    /// When a file that isn't a picture was refused, to shake the strip.
    refused: Option<Instant>,
}

impl EffectDraft {
    fn new(window: &mut Window, cx: &mut Context<ProfileItemsView>) -> Self {
        let wgsl = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(6, 16).placeholder("fn shade(uv: vec2f) -> vec4f { ... }")
        });
        Self {
            wgsl,
            pictures: Vec::new(),
            shader_open: false,
            resting: String::new(),
            typed: 0,
            initial: String::new(),
            initial_urls: Vec::new(),
            refused: None,
        }
    }

    fn urls(&self) -> Vec<String> {
        self.pictures
            .iter()
            .map(|p| match p {
                DraftPicture::Uploaded { url, .. } => url.clone(),
                DraftPicture::File(path) => path.to_string_lossy().into_owned(),
            })
            .collect()
    }

    /// Whether its pictures differ from the ones it started with, or any is a file to upload.
    fn pictures_changed(&self) -> bool {
        self.urls() != self.initial_urls || self.pictures.iter().any(|p| matches!(p, DraftPicture::File(_)))
    }
}

/// Something about to be added.
enum Draft {
    /// A picture from this computer, shown from the file until it's uploaded.
    Decoration {
        path: PathBuf,
    },
    /// A wide picture from this computer, the same way.
    Nameplate {
        path: PathBuf,
    },
    Effect,
}

/// An item's name and line, changed in place, and what they were filled with.
struct Inline {
    name: Entity<InputState>,
    description: Entity<InputState>,
    filled: (String, String),
    saving: bool,
    /// Refused (an empty name): shakes once.
    shook: Option<Instant>,
}

pub struct ProfileItemsView {
    core: Arc<Core>,
    key: String,
    scope: Scope,
    /// The instance's list is read again on opening; until then it shimmers.
    loaded: bool,
    draft: Option<Draft>,
    /// A new item's name and line, and an effect's spec.
    name: Entity<InputState>,
    description: Entity<InputState>,
    spec: Entity<TextareaState>,
    /// An effect getting a new spec, and the box it's typed in.
    replacing: Option<String>,
    replace: Entity<TextareaState>,
    /// The new effect's shader and pictures, and the changed one's.
    fresh: EffectDraft,
    changing: EffectDraft,
    /// The item asked about before it's deleted.
    confirming: Option<String>,
    /// The row under the pointer, whose effect plays.
    hovered: Option<String>,
    busy: bool,
    error: Option<String>,
    /// When a file that isn't a picture was refused, to shake the drop card,
    /// and whether it was the nameplate's.
    refused: Option<Instant>,
    refused_plate: bool,
    /// The prompt just copied (its button's id) and which copy it was, so only the latest clears.
    copied: Option<(&'static str, u64)>,
    copies: u64,
    inline: HashMap<String, Inline>,
    effects: HashMap<String, Entity<EffectView>>,
    _subscriptions: Vec<Subscription>,
}

impl ProfileItemsView {
    pub fn new(core: Arc<Core>, key: String, scope: Scope, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.profileItems.namePlaceholder")));
        let description = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("serversettings.profileItems.descriptionPlaceholder")));
        let spec = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(6, 14)
                .placeholder(t("serversettings.profileItems.specPlaceholder"))
        });
        let replace = cx.new(|cx| TextareaState::new(window, cx).auto_grow(6, 14));
        let notify = |this: &mut Self, _: Entity<_>, e: &InputEvent, cx: &mut Context<Self>| {
            if matches!(e, InputEvent::Change) {
                this.error = None;
                cx.notify();
            }
        };
        let fresh = EffectDraft::new(window, cx);
        let changing = EffectDraft::new(window, cx);
        let subscriptions = vec![
            cx.subscribe(&name, notify),
            cx.subscribe(&description, notify),
            cx.subscribe_in(&spec, window, |this: &mut Self, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::Change) {
                    this.error = None;
                    this.take_shader(false, window, cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&replace, window, |this: &mut Self, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::Change) {
                    this.error = None;
                    this.take_shader(true, window, cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&fresh.wgsl, |this: &mut Self, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.error = None;
                    this.rest_shader(false, cx);
                }
            }),
            cx.subscribe(&changing.wgsl, |this: &mut Self, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.error = None;
                    this.rest_shader(true, cx);
                }
            }),
        ];
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
        let mut view = Self {
            core,
            key,
            scope,
            loaded: true,
            draft: None,
            name,
            description,
            spec,
            replacing: None,
            replace,
            fresh,
            changing,
            confirming: None,
            hovered: None,
            busy: false,
            error: None,
            refused: None,
            refused_plate: false,
            copied: None,
            copies: 0,
            inline: HashMap::new(),
            effects: HashMap::new(),
            _subscriptions: subscriptions,
        };
        // The instance's list is kept from sign-in; read it again here so an admin sees what's there now.
        if view.scope == Scope::Instance {
            view.loaded = false;
            let (core, key) = (view.core.clone(), view.key.clone());
            view.run(
                cx,
                async move {
                    core.refresh_instance_items(&key).await;
                    Ok(())
                },
                |this, _, cx| {
                    this.loaded = true;
                    cx.notify();
                },
            );
        }
        view
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

    fn me(&self) -> Option<pb::User> {
        self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.me.clone()))
    }

    fn items(&self) -> Vec<pb::ProfileItem> {
        self.core.shared.read(|s| match s.instance(&self.key) {
            Some(i) => match &self.scope {
                Scope::Instance => i.profile_items.clone(),
                Scope::Server(sid) => i.server_list(sid).to_vec(),
            },
            None => Vec::new(),
        })
    }

    /// Starts a new item: clears the fields and opens its panel.
    fn start(&mut self, draft: Draft, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.description.update(cx, |s, cx| s.set_value("", window, cx));
        if matches!(draft, Draft::Effect) {
            self.spec.update(cx, |s, cx| s.set_value("", window, cx));
            self.fresh.wgsl.update(cx, |s, cx| s.set_value("", window, cx));
            self.fresh.pictures.clear();
            self.fresh.shader_open = false;
            self.fresh.resting.clear();
            self.fresh.refused = None;
        }
        self.draft = Some(draft);
        self.error = None;
        cx.notify();
    }

    /// A picture dropped or picked: the first one that is a picture.
    fn take_pictures(&mut self, paths: Vec<PathBuf>, plate: bool, window: &mut Window, cx: &mut Context<Self>) {
        let picture = paths.iter().find(|p| p.file_name().and_then(|n| picture_type(&n.to_string_lossy())).is_some());
        match picture {
            Some(path) => {
                let name = path.file_name().map(|n| name_from_file(&n.to_string_lossy())).unwrap_or_default();
                let draft = if plate {
                    Draft::Nameplate { path: path.clone() }
                } else {
                    Draft::Decoration { path: path.clone() }
                };
                self.start(draft, name, window, cx);
            }
            None if !paths.is_empty() => {
                self.refused = Some(Instant::now());
                self.refused_plate = plate;
                self.error = Some(t("serversettings.profileItems.badType"));
                cx.notify();
            }
            None => {}
        }
    }

    fn pick_picture(&mut self, plate: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("desktop.account.choosePicture").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let _ = this.update_in(cx, |this, window, cx| this.take_pictures(paths, plate, window, cx));
        })
        .detach();
    }

    /// Opens a `.json` file into the spec box (the new effect's, or the one being replaced).
    fn open_spec(&mut self, replacing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("desktop.profileItems.chooseEffect").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let text = read_effect(&path);
            let _ = this.update_in(cx, |this, window, cx| {
                match text {
                    Ok(text) => {
                        let target = if replacing { this.replace.clone() } else { this.spec.clone() };
                        target.update(cx, |s, cx| s.set_value(text, window, cx));
                        this.take_shader(replacing, window, cx);
                        if !replacing && this.name.read(cx).value().trim().is_empty() {
                            this.name.update(cx, |s, cx| s.set_value(name_from_file(&file), window, cx));
                        }
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fx_draft(&mut self, replacing: bool) -> &mut EffectDraft {
        if replacing { &mut self.changing } else { &mut self.fresh }
    }

    /// Whether the instance takes pictures and a shader in effects (older ones would refuse them).
    fn media_here(&self) -> bool {
        self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| i.has(MEDIA)))
    }

    /// A spec with a shader in it (a file, a paste, an AI's answer) has its code moved to the shader's own field.
    fn take_shader(&mut self, replacing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let state = if replacing { self.replace.clone() } else { self.spec.clone() };
        let (json, wgsl) = split_shader(&state.read(cx).value());
        if wgsl.is_empty() {
            return;
        }
        state.update(cx, |s, cx| s.set_value(json, window, cx));
        self.set_wgsl(replacing, wgsl, window, cx);
    }

    /// Puts code in the shader's field (opening it), for the preview at once.
    fn set_wgsl(&mut self, replacing: bool, wgsl: String, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.fx_draft(replacing);
        draft.shader_open = draft.shader_open || !wgsl.trim().is_empty();
        draft.resting = wgsl.clone();
        draft.typed += 1;
        let state = draft.wgsl.clone();
        state.update(cx, |s, cx| s.set_value(wgsl, window, cx));
        self.error = None;
        cx.notify();
    }

    /// The shader's code changed: the preview takes it once typing rests, so it isn't compiled on every key.
    fn rest_shader(&mut self, replacing: bool, cx: &mut Context<Self>) {
        let draft = self.fx_draft(replacing);
        draft.typed += 1;
        let typed = draft.typed;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHADER_REST).await;
            let _ = this.update(cx, |this, cx| {
                let draft = this.fx_draft(replacing);
                if draft.typed == typed {
                    let code = draft.wgsl.read(cx).value().to_string();
                    let draft = this.fx_draft(replacing);
                    draft.resting = code;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The spec as it would be sent: the JSON with the shader's code put back in.
    fn effect_text(&self, replacing: bool, cx: &Context<Self>) -> String {
        let (state, draft) = if replacing { (&self.replace, &self.changing) } else { (&self.spec, &self.fresh) };
        join_shader(&state.read(cx).value(), &draft.wgsl.read(cx).value())
    }

    /// The draft checked as the preview plays it (with the code that has rested).
    fn checked(&self, replacing: bool, cx: &Context<Self>) -> Result<Spec, crate::core::profile_items::EffectProblem> {
        let (state, draft) = if replacing { (&self.replace, &self.changing) } else { (&self.spec, &self.fresh) };
        let pictures: Vec<fx::Picture> = draft.pictures.iter().map(DraftPicture::picture).collect();
        check_effect_text(&join_shader(&state.read(cx).value(), &draft.resting), &pictures)
    }

    /// Pictures dropped on the strip or picked: the ones that are pictures, up to the most an effect has.
    fn add_effect_pictures(&mut self, replacing: bool, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let fit: Vec<PathBuf> = paths
            .iter()
            .filter(|p| p.file_name().and_then(|n| picture_type(&n.to_string_lossy())).is_some())
            .cloned()
            .collect();
        if !paths.is_empty() && fit.is_empty() {
            self.error = Some(t("serversettings.profileItems.badType"));
            self.fx_draft(replacing).refused = Some(Instant::now());
            cx.notify();
            return;
        }
        let draft = self.fx_draft(replacing);
        let room = fx::MAX_PICTURES.saturating_sub(draft.pictures.len());
        let full = fit.len() > room;
        for path in fit.into_iter().take(room) {
            if !draft.pictures.contains(&DraftPicture::File(path.clone())) {
                draft.pictures.push(DraftPicture::File(path));
            }
        }
        self.error = full.then(|| {
            t_with("serversettings.profileItems.picturesFull", &[("count", Arg::Num(fx::MAX_PICTURES as i64))])
        });
        cx.notify();
    }

    fn pick_effect_pictures(&mut self, replacing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t("serversettings.profileItems.addPicture").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let _ = this.update(cx, |this, cx| this.add_effect_pictures(replacing, paths, cx));
        })
        .detach();
    }

    /// A built-in effect's layers into the new effect's box, and its name when there's none yet.
    fn start_from(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = starter_text(id) else { return };
        self.spec.update(cx, |s, cx| s.set_value(text, window, cx));
        if self.name.read(cx).value().trim().is_empty() {
            let name = crate::ui::profile_effect::effect_name(id);
            self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        }
        self.error = None;
        cx.notify();
    }

    /// Copies a prompt for an AI; its button says so for a moment.
    fn copy_prompt(&mut self, id: &'static str, text: &'static str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        self.copies += 1;
        let copy = self.copies;
        self.copied = Some((id, copy));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(2400)).await;
            let _ = this.update(cx, |this, cx| {
                if this.copied.is_some_and(|(_, c)| c == copy) {
                    this.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Saves the decoration template, or the nameplate's, where the person says.
    fn save_template(&mut self, plate: bool, cx: &mut Context<Self>) {
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let name = if plate { "fuwa-nameplate-template.png" } else { "fuwa-decoration-template.png" };
        let path = cx.prompt_for_new_path(&dir, Some(name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let file = if plate {
                crate::ui::png::png(NAMEPLATE_WIDTH, NAMEPLATE_HEIGHT, &nameplate_template_pixels(), true)
            } else {
                crate::ui::png::png(TEMPLATE_SIZE, TEMPLATE_SIZE, &template_pixels(), true)
            };
            if std::fs::write(&path, file).is_err() {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(t("desktop.profileItems.cantSaveTemplate"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else { return };
        let name: String = self.name.read(cx).value().trim().chars().take(NAME_MAX).collect();
        let description: String = self.description.read(cx).value().trim().chars().take(DESCRIPTION_MAX).collect();
        if name.is_empty() || self.busy {
            return;
        }
        let new = match draft {
            Draft::Decoration { path } => Err((path.clone(), false)),
            Draft::Nameplate { path } => Err((path.clone(), true)),
            Draft::Effect => {
                let text = self.effect_text(false, cx);
                let pictures: Vec<fx::Picture> = self.fresh.pictures.iter().map(DraftPicture::picture).collect();
                if check_effect_text(&text, &pictures).is_err() {
                    return;
                }
                Ok((text, self.fresh.pictures.clone()))
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let (core, key, scope) = (self.core.clone(), self.key.clone(), self.scope.clone());
        self.run(
            cx,
            async move {
                let (new, urls) = match new {
                    Ok((spec, pictures)) => {
                        let urls = core.send_pictures(&key, &scope, pictures).await?;
                        (NewItem::Effect { spec, pictures: urls.clone() }, urls)
                    }
                    Err((path, plate)) => {
                        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let content_type = picture_type(&file).unwrap_or("image/png").to_owned();
                        let bytes = crate::core::account::read_picture(&path).await?;
                        let new = if plate {
                            NewItem::Nameplate { content_type, bytes }
                        } else {
                            NewItem::Decoration { content_type, bytes }
                        };
                        (new, Vec::new())
                    }
                };
                Ok((urls, core.add_profile_item(&key, &scope, &name, &description, new).await))
            },
            |this, result, cx| {
                this.busy = false;
                match result {
                    Ok((_, Ok(_))) => this.draft = None,
                    Ok((urls, Err(err))) => {
                        // Uploaded once: trying again after a refusal doesn't upload them again.
                        uploaded(&mut this.fresh, urls);
                        this.error = Some(err.message);
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
    }

    fn start_replacing(&mut self, item: &pb::ProfileItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.replacing.as_deref() == Some(item.id.as_str()) {
            self.replacing = None;
        } else {
            self.replacing = Some(item.id.clone());
            let (json, wgsl) = split_shader(&item.effect);
            let json = if wgsl.is_empty() { pretty(&item.effect) } else { json };
            self.changing.initial = join_shader(&json, &wgsl);
            self.changing.initial_urls = item.pictures.iter().map(|p| p.url.clone()).collect();
            self.changing.pictures = item
                .pictures
                .iter()
                .map(|p| DraftPicture::Uploaded { url: p.url.clone(), animated: p.animated })
                .collect();
            self.changing.shader_open = false;
            self.changing.refused = None;
            self.replace.update(cx, |s, cx| s.set_value(json, window, cx));
            self.set_wgsl(true, wgsl, window, cx);
        }
        self.error = None;
        cx.notify();
    }

    /// Whether the effect being changed differs from what it has.
    fn replace_changed(&self, cx: &Context<Self>) -> bool {
        self.effect_text(true, cx) != self.changing.initial || self.changing.pictures_changed()
    }

    fn save_replace(&mut self, item: pb::ProfileItem, cx: &mut Context<Self>) {
        let text = self.effect_text(true, cx);
        let pictures: Vec<fx::Picture> = self.changing.pictures.iter().map(DraftPicture::picture).collect();
        if check_effect_text(&text, &pictures).is_err() || !self.replace_changed(cx) || self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let (core, key, scope) = (self.core.clone(), self.key.clone(), self.scope.clone());
        let send = self.changing.pictures_changed().then(|| self.changing.pictures.clone());
        let run = async move {
            let urls = match send {
                Some(pictures) => Some(core.send_pictures(&key, &scope, pictures).await?),
                None => None,
            };
            let change = pb::ProfileItemChange {
                effect: Some(text),
                pictures: urls.clone().map(|urls| pb::ProfileItemPictureUrls { urls }),
                ..Default::default()
            };
            Ok((urls, core.change_profile_item(&key, &scope, &item.id, change).await))
        };
        self.run(cx, run, |this, r, cx| {
            this.busy = false;
            match r {
                Ok((_, Ok(_))) => this.replacing = None,
                Ok((urls, Err(err))) => {
                    uploaded(&mut this.changing, urls.unwrap_or_default());
                    this.error = Some(err.message);
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// A name or line left (Enter or losing focus): saved when it changed, put back when a name
    /// was emptied or the instance says no.
    fn save_inline(&mut self, id: String, name_field: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.inline.get_mut(&id) else { return };
        let state = if name_field { row.name.clone() } else { row.description.clone() };
        let before = if name_field { row.filled.0.clone() } else { row.filled.1.clone() };
        let most = if name_field { NAME_MAX } else { DESCRIPTION_MAX };
        let next: String = state.read(cx).value().trim().chars().take(most).collect();
        if next == before {
            if state.read(cx).value() != before {
                state.update(cx, |s, cx| s.set_value(before, window, cx));
            }
            return;
        }
        if name_field && next.is_empty() {
            row.shook = Some(Instant::now());
            state.update(cx, |s, cx| s.set_value(before, window, cx));
            cx.notify();
            return;
        }
        row.saving = true;
        let change = if name_field {
            pb::ProfileItemChange { name: Some(next), ..Default::default() }
        } else {
            pb::ProfileItemChange { description: Some(next), ..Default::default() }
        };
        let (core, key, scope) = (self.core.clone(), self.key.clone(), self.scope.clone());
        let item_id = id.clone();
        self.run(
            cx,
            async move { core.change_profile_item(&key, &scope, &item_id, change).await },
            move |this, r, cx| {
                if let Some(row) = this.inline.get_mut(&id) {
                    row.saving = false;
                    if r.is_err() {
                        // Filled again from the list on the next draw.
                        row.filled = (String::new(), String::new());
                    }
                }
                if let Err(err) = r {
                    this.error = Some(err.message);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.confirming = None;
        let (core, key, scope) = (self.core.clone(), self.key.clone(), self.scope.clone());
        self.run(cx, async move { core.delete_profile_item(&key, &scope, &id).await }, |this, result, cx| {
            if let Err(err) = result {
                this.error = Some(err.message);
            }
            cx.notify();
        });
        cx.notify();
    }

    /// The fields an item's name and line are changed in, made once and kept in step with the list.
    fn inline_for(&mut self, item: &pb::ProfileItem, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inline.contains_key(&item.id) {
            let name = cx.new(|cx| InputState::new(window, cx));
            let description =
                cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.profileItems.addDescription")));
            for (state, is_name) in [(&name, true), (&description, false)] {
                let id = item.id.clone();
                cx.subscribe_in(state, window, move |this: &mut Self, _, e: &InputEvent, window, cx| match e {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.save_inline(id.clone(), is_name, window, cx)
                    }
                    _ => {}
                })
                .detach();
            }
            self.inline.insert(
                item.id.clone(),
                Inline { name, description, filled: (String::new(), String::new()), saving: false, shook: None },
            );
        }
        let row = self.inline.get_mut(&item.id).expect("made above");
        if row.filled.0 != item.name {
            row.filled.0 = item.name.clone();
            let v = item.name.clone();
            row.name.update(cx, |s, cx| s.set_value(v, window, cx));
        }
        if row.filled.1 != item.description {
            row.filled.1 = item.description.clone();
            let v = item.description.clone();
            row.description.update(cx, |s, cx| s.set_value(v, window, cx));
        }
    }

    // ───────────────────────── Drawing ─────────────────────────

    fn add_card(
        &self,
        id: &'static str,
        glyph: &'static str,
        title: String,
        about: String,
        p: &Palette,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let (hover_bg, hover_border) = (alpha(p.primary, 0.05), alpha(p.primary, 0.5));
        // The picture tips one way, the sparkles grow and tip the other, while pointed at.
        let sparkles = glyph == "sparkles";
        div()
            .id(id)
            .group(id)
            .flex_1()
            .min_w_0()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .p(px(20.0))
            .rounded(radius_3xl())
            .border_2()
            .border_dashed()
            .border_color(p.border)
            .text_center()
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg).border_color(hover_border).translate_y(px(-2.0)))
            .active(|s| s.scale(0.98))
            .child(
                div()
                    .size(px(44.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.primary, 0.15))
                    .text_color(p.primary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("pi-add-glyph")
                            .group_hover(id, move |s| {
                                if sparkles {
                                    s.scale(1.1).rotate(gpui_kit::radians(-12f32.to_radians()))
                                } else {
                                    s.rotate(gpui_kit::radians(12f32.to_radians()))
                                }
                            })
                            .child(icon(glyph).size(px(20.0))),
                    ),
            )
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title))
            .child(div().max_w(px(320.0)).text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(about))
    }

    /// A small helper under something to add: a glyph and a few words, in the primary color.
    fn helper(
        id: &'static str,
        glyph: &'static str,
        label: String,
        tip: Option<String>,
        color: gpui_kit::Rgba,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .cursor_pointer()
            .hover(|s| s.underline())
            .active(|s| s.scale(0.94))
            .when_some(tip, |el, tip| {
                el.tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
            })
            .child(icon(glyph).size(px(14.0)))
            .child(label)
    }

    /// Copies a prompt for an AI, then says it's copied for a moment.
    fn ask_ai(
        &self,
        id: &'static str,
        text: &'static str,
        tip: String,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self.copied.is_some_and(|(at, _)| at == id);
        motion::rise(
            Self::helper(
                id,
                if copied { "check" } else { "wand-sparkles" },
                t(if copied { "serversettings.profileItems.askAiCopied" } else { "serversettings.profileItems.askAi" }),
                Some(tip),
                if copied { p.success } else { p.primary },
            )
            .on_click(cx.listener(move |this, _, _, cx| this.copy_prompt(id, text, cx))),
            SharedString::from(format!("{id}-{copied}")),
            Duration::ZERO,
            12.0,
        )
        .into_any_element()
    }

    /// Opens a kind's guide in the browser.
    fn guide(id: &'static str, url: &'static str, p: &Palette) -> AnyElement {
        Self::helper(id, "book-open", t("serversettings.profileItems.guide"), None, p.primary)
            .on_click(move |_, _, cx| cx.open_url(url))
            .into_any_element()
    }

    fn helpers(children: Vec<AnyElement>) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .items_center()
            .gap_x(px(12.0))
            .gap_y(px(4.0))
            .px(px(8.0))
            .children(children)
            .into_any_element()
    }

    /// The built-in effects as specs to start from.
    fn starters(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (border, hover_border, hover_bg) = (p.border, alpha(p.primary, 0.5), alpha(p.primary, 0.1));
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .mr(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.profileItems.startFrom")),
            )
            .children(fx::IDS.into_iter().map(|id| {
                let tip = crate::ui::profile_effect::about(id).unwrap_or_default();
                div()
                    .id(SharedString::from(format!("pi-start-{id}")))
                    .rounded_full()
                    .border_1()
                    .border_color(border)
                    .bg(alpha(p.background, 0.6))
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.border_color(hover_border).bg(hover_bg).translate_y(px(-1.0)))
                    .active(|s| s.scale(0.94))
                    .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                    .when(!self.busy, |el| {
                        el.on_click(cx.listener(move |this, _, window, cx| this.start_from(id, window, cx)))
                    })
                    .child(crate::ui::profile_effect::effect_name(id))
            }))
            .into_any_element()
    }

    fn adders(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (primary, drag_bg) = (p.primary, alpha(p.primary, 0.1));
        let decoration = self
            .add_card(
                "pi-add-decoration",
                "image-plus",
                t("serversettings.profileItems.addDecoration"),
                t("serversettings.profileItems.addDecorationHint"),
                p,
            )
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drag_bg).border_color(primary))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.take_pictures(paths.paths().to_vec(), false, window, cx)
            }))
            .on_click(cx.listener(|this, _, window, cx| this.pick_picture(false, window, cx)));
        let refused = self.refused.filter(|at| at.elapsed() < Duration::from_millis(500));
        let shaken = |card: gpui_kit::Stateful<gpui_kit::Div>, plate: bool| -> AnyElement {
            match refused.filter(|_| self.refused_plate == plate) {
                Some(at) => card
                    .with_animation(
                        SharedString::from(format!("pi-refused-{at:?}")),
                        gpui_kit::Animation::new(Duration::from_millis(400)),
                        |el, t| el.left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t))),
                    )
                    .into_any_element(),
                None => card.into_any_element(),
            }
        };
        let decoration = shaken(decoration, false);
        let effect = self
            .add_card(
                "pi-add-effect",
                "sparkles",
                t("serversettings.profileItems.addEffect"),
                t("serversettings.profileItems.addEffectHint"),
                p,
            )
            .on_click(cx.listener(|this, _, window, cx| this.start(Draft::Effect, String::new(), window, cx)));
        let decoration_help = Self::helpers(vec![
            Self::helper(
                "pi-template",
                "download",
                t("serversettings.profileItems.template"),
                Some(t("serversettings.profileItems.templateHint")),
                p.primary,
            )
            .on_click(cx.listener(|this, _, _, cx| this.save_template(false, cx)))
            .into_any_element(),
            self.ask_ai(
                "pi-ask-decoration",
                DECORATION_PROMPT,
                t("serversettings.profileItems.askAiDecorationHint"),
                p,
                cx,
            ),
            Self::guide("pi-guide-decoration", DECORATION_DOCS, p),
        ]);
        let effect_help = Self::helpers(vec![
            self.ask_ai("pi-ask-effect", EFFECT_PROMPT, t("serversettings.profileItems.askAiEffectHint"), p, cx),
            Self::guide("pi-guide-effect", EFFECT_DOCS, p),
        ]);
        let column = |card: AnyElement, help: AnyElement| {
            div().flex_1().min_w_0().flex().flex_col().gap(px(8.0)).child(card).child(help)
        };
        let both = div()
            .flex()
            .gap(px(12.0))
            .child(column(decoration, decoration_help))
            .child(column(effect.into_any_element(), effect_help));
        // Nameplates, on an instance that has them, take a line of their own: the card is as wide as they are.
        if !self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| i.has("nameplates"))) {
            return both.into_any_element();
        }
        let nameplate = self
            .add_card(
                "pi-add-nameplate",
                "rectangle-horizontal",
                t("serversettings.profileItems.addNameplate"),
                t("serversettings.profileItems.addNameplateHint"),
                p,
            )
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drag_bg).border_color(primary))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.take_pictures(paths.paths().to_vec(), true, window, cx)
            }))
            .on_click(cx.listener(|this, _, window, cx| this.pick_picture(true, window, cx)));
        let nameplate = shaken(nameplate, true);
        let nameplate_help = Self::helpers(vec![
            Self::helper(
                "pi-nameplate-template",
                "download",
                t("serversettings.profileItems.template"),
                Some(t("serversettings.profileItems.nameplateTemplateHint")),
                p.primary,
            )
            .on_click(cx.listener(|this, _, _, cx| this.save_template(true, cx)))
            .into_any_element(),
            self.ask_ai(
                "pi-ask-nameplate",
                NAMEPLATE_PROMPT,
                t("serversettings.profileItems.askAiNameplateHint"),
                p,
                cx,
            ),
            Self::guide("pi-guide-nameplate", NAMEPLATE_DOCS, p),
        ]);
        div().flex().flex_col().gap(px(12.0)).child(both).child(column(nameplate, nameplate_help)).into_any_element()
    }

    /// A panel for something about to be added: its preview and fields, then Cancel or Add.
    #[allow(clippy::too_many_arguments)]
    fn panel(
        &self,
        id: &'static str,
        title: String,
        body: Vec<AnyElement>,
        can_add: bool,
        add_label: String,
        p: &Palette,
        on_add: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        on_cancel: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.busy;
        let mut add = button(SharedString::from(format!("{id}-add")), add_label, None, Look::Primary, false, p)
            .rounded_full()
            .font_weight(FontWeight::BOLD)
            .child(icon(if busy { "loader-circle" } else { "check" }).size(px(16.0)));
        // The glyph goes first, as on the web.
        add = add.flex_row_reverse();
        let add = if can_add && !busy {
            add.on_click(cx.listener(move |this, _, _, cx| on_add(this, cx))).into_any_element()
        } else {
            add.opacity(0.5).cursor_default().into_any_element()
        };
        let cancel = button(
            SharedString::from(format!("{id}-cancel")),
            t("serversettings.profileItems.cancel"),
            None,
            Look::Ghost,
            false,
            p,
        )
        .rounded_full()
        .when(!busy, |el| el.on_click(cx.listener(move |this, _, _, cx| on_cancel(this, cx))));
        let error = self.error.clone();
        motion::slide_in(
            div()
                .relative()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .overflow_hidden()
                .rounded(radius_3xl())
                .border_1()
                .border_color(p.border)
                .bg(p.background)
                .p(px(16.0))
                .shadow(shadow_sm())
                .when(busy && (id == "pi-new-decoration" || id == "pi-new-nameplate"), |el| {
                    el.child(div().absolute().top_0().left_0().h(px(4.0)).bg(p.primary).with_animation(
                        SharedString::from(format!("{id}-progress")),
                        gpui_kit::Animation::new(Duration::from_millis(1600)).with_easing(gpui_kit::ease_out_quint()),
                        |el, t| el.w(gpui_kit::relative(t * 0.9)),
                    ))
                })
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(title))
                .children(body)
                .when_some(error, |el, e| {
                    el.child(div().text_sm().text_color(p.destructive).child(crate::ui::instance_home::capitalized(&e)))
                })
                .child(div().flex().justify_end().gap(px(8.0)).child(cancel).child(add)),
            SharedString::from(id),
            12.0,
        )
        .into_any_element()
    }

    fn name_fields(&self, p: &Palette) -> AnyElement {
        let boxed = |state: &Entity<InputState>| {
            div()
                .h(px(40.0))
                .w_full()
                .rounded(radius_xl())
                .border_1()
                .border_color(p.border)
                .bg(p.background)
                .px(px(1.0))
                .flex()
                .items_center()
                .text_sm()
                .shadow(shadow_sm())
                .child(div().flex_1().min_w_0().child(Input::new(state).appearance(false)))
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(boxed(&self.name))
            .child(boxed(&self.description))
            .into_any_element()
    }

    /// An effect's own pictures, in order: each with the number its layers name
    /// it by (`"image": 0`), to take out, and a tile to add more (picked or
    /// dropped) up to the most an effect has.
    fn effect_pictures(&self, replacing: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let draft = if replacing { &self.changing } else { &self.fresh };
        let busy = self.busy;
        let full = draft.pictures.len() >= fx::MAX_PICTURES;
        let (primary, drag_bg) = (p.primary, alpha(p.primary, 0.1));
        let which = if replacing { "replace" } else { "new" };
        let mut strip = div()
            .id(SharedString::from(format!("pi-pictures-{which}")))
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .p(px(8.0))
            .rounded(radius_2xl())
            .border_1()
            .border_dashed()
            .border_color(p.border)
            .when(!busy && !full, |el| {
                el.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drag_bg).border_color(primary)).on_drop(
                    cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        this.add_effect_pictures(replacing, paths.paths().to_vec(), cx)
                    }),
                )
            });
        for (n, shown) in draft.pictures.iter().enumerate() {
            let (key, face) = match shown {
                DraftPicture::Uploaded { url, .. } => (
                    url.clone(),
                    crate::ui::widgets::picture(url.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                ),
                DraftPicture::File(path) => {
                    let key = path.to_string_lossy().into_owned();
                    let face = img(path.clone())
                        .id(SharedString::from(format!("picture|{key}")))
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element();
                    (key, face)
                }
            };
            let group = SharedString::from(format!("pi-pic-{which}-{n}"));
            let remove_tip = t_with("serversettings.profileItems.removePicture", &[("n", Arg::Num(n as i64))]);
            let (bg, fg, red) = (p.background, p.muted_foreground, p.destructive);
            let gone = shown.clone();
            let tile = div()
                .id(group.clone())
                .group(group.clone())
                .relative()
                .size(px(56.0))
                .rounded(radius_xl())
                .bg(alpha(p.muted, 0.6))
                .child(div().size_full().p(px(4.0)).child(face))
                .child(
                    div().absolute().bottom(px(-4.0)).left_0().right_0().flex().justify_center().child(
                        div()
                            .rounded_full()
                            .bg(bg)
                            .px(px(6.0))
                            .text_size(px(10.4))
                            .font_weight(FontWeight::BOLD)
                            .font_family(crate::ui::theme::mono())
                            .shadow(shadow_sm())
                            .child(n.to_string()),
                    ),
                )
                .child(
                    div()
                        .id("pi-pic-remove")
                        .absolute()
                        .top(px(-6.0))
                        .right(px(-6.0))
                        .size(px(20.0))
                        .rounded_full()
                        .bg(bg)
                        .text_color(fg)
                        .shadow(shadow_sm())
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.0)
                        .group_hover(group, |s| s.opacity(1.0))
                        .hover(move |s| s.text_color(red))
                        .when(!busy, |el| {
                            el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                this.fx_draft(replacing).pictures.retain(|p| p != &gone);
                                this.error = None;
                                cx.notify();
                            }))
                        })
                        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(remove_tip.clone()).build(window, cx))
                        .child(icon("x").size(px(12.0))),
                );
            strip = strip.child(motion::grow_in(tile, SharedString::from(format!("pi-pic-in-{which}-{key}")), 0.6));
        }
        if !full {
            let (hover_bg, tip) = (alpha(p.primary, 0.2), t("serversettings.profileItems.addPicture"));
            strip =
                strip.child(
                    div()
                        .id(SharedString::from(format!("pi-pic-add-{which}")))
                        .size(px(56.0))
                        .rounded(radius_xl())
                        .bg(alpha(p.primary, 0.1))
                        .text_color(p.primary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                        .when(busy, |el| el.opacity(0.5))
                        .when(!busy, |el| {
                            el.cursor_pointer()
                                .hover(move |s| s.bg(hover_bg).translate_y(px(-2.0)))
                                .active(|s| s.scale(0.94))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.pick_effect_pictures(replacing, window, cx)
                                }))
                        })
                        .child(icon("image-plus").size(px(20.0))),
                );
        }
        let strip: AnyElement = match draft.refused.filter(|at| at.elapsed() < Duration::from_millis(500)) {
            Some(at) => strip
                .with_animation(
                    SharedString::from(format!("pi-pics-refused-{at:?}")),
                    gpui_kit::Animation::new(Duration::from_millis(400)),
                    |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t))),
                )
                .into_any_element(),
            None => strip.into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.0))
                    .text_xs()
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.profileItems.pictures")))
                    .child(div().text_color(p.muted_foreground).child(t("serversettings.profileItems.picturesHint"))),
            )
            .child(strip)
            .into_any_element()
    }

    /// An effect's shader: WGSL in the custom shaders' format, drawn under the
    /// particles. Closed until someone adds one; while it's open it says what
    /// happened when this computer last ran it (from the painter, by its
    /// code), and offers the backdrop's starters to begin from.
    fn shader_field(&self, replacing: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let draft = if replacing { &self.changing } else { &self.fresh };
        let busy = self.busy;
        let which = if replacing { "replace" } else { "new" };
        let code = draft.wgsl.read(cx).value().to_string();
        if !draft.shader_open && code.trim().is_empty() {
            let (hover_border, hover_bg) = (alpha(p.primary, 0.5), alpha(p.primary, 0.05));
            return div()
                .flex()
                .child(
                    div()
                        .id(SharedString::from(format!("pi-shader-add-{which}")))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded_full()
                        .border_1()
                        .border_dashed()
                        .border_color(p.border)
                        .px(px(12.0))
                        .py(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .when(busy, |el| el.opacity(0.5))
                        .when(!busy, |el| {
                            el.cursor_pointer()
                                .hover(move |s| s.border_color(hover_border).bg(hover_bg).translate_y(px(-1.0)))
                                .active(|s| s.scale(0.97))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.fx_draft(replacing).shader_open = true;
                                    cx.notify();
                                }))
                        })
                        .child(icon("code").size(px(14.0)))
                        .child(t("serversettings.profileItems.addShader"))
                        .child(
                            div()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.profileItems.addShaderHint")),
                        ),
                )
                .into_any_element();
        }
        let id = (!code.trim().is_empty()).then(|| shader_id(&code));
        let nothing = t("appsettings.shader.nothing");
        // What to say: its text, whether it's bad news, and whether it can be tried again.
        let note: Option<(String, bool, bool)> = if code.trim().is_empty() {
            None
        } else if let Some(problem) = shader_problem(&code) {
            Some((problem, true, false))
        } else {
            match id.as_deref().and_then(status::status) {
                Some(ShaderStatus::Broken { message, line }) => Some((
                    match line {
                        Some(line) => format!(
                            "{} {message}",
                            t_with("appsettings.shader.line", &[("line", Arg::Num(line as i64))])
                        ),
                        None => message,
                    },
                    true,
                    false,
                )),
                Some(ShaderStatus::Slow { .. }) => {
                    Some((t_with("appsettings.shader.slow", &[("fallback", Arg::Str(&nothing))]), true, true))
                }
                Some(ShaderStatus::Stopped) => {
                    Some((t_with("appsettings.shader.stopped", &[("fallback", Arg::Str(&nothing))]), true, true))
                }
                Some(ShaderStatus::Running { scale, .. }) => Some((
                    t(if scale > 0.6 {
                        "appsettings.shader.running"
                    } else if scale > 0.4 {
                        "appsettings.shader.runningHalf"
                    } else {
                        "appsettings.shader.runningThird"
                    }),
                    false,
                    false,
                )),
                None if gpu::gpu() == Gpu::Missing => {
                    Some((t_with("desktop.shader.noGpu", &[("fallback", Arg::Str(&nothing))]), true, false))
                }
                None => None,
            }
        };
        let (border, hover_border, hover_bg) = (p.border, alpha(p.primary, 0.5), alpha(p.primary, 0.1));
        let starters = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .mr(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.profileItems.startFrom")),
            )
            .children(STARTERS.iter().map(|starter| {
                let tip = t(starter.hint);
                let code = starter.code;
                div()
                    .id(SharedString::from(format!("pi-shader-start-{which}-{}", starter.id)))
                    .rounded_full()
                    .border_1()
                    .border_color(border)
                    .bg(alpha(p.background, 0.6))
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.border_color(hover_border).bg(hover_bg).translate_y(px(-1.0)))
                    .active(|s| s.scale(0.94))
                    .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                    .when(!busy, |el| {
                        el.on_click(
                            cx.listener(move |this, _, window, cx| this.set_wgsl(replacing, code.into(), window, cx)),
                        )
                    })
                    .child(starter.name)
            }));
        let state = draft.wgsl.clone();
        let remove_tip = t("serversettings.profileItems.removeShader");
        let remove =
            round_icon(SharedString::from(format!("pi-shader-remove-{which}")), "trash", p, p.destructive.into())
                .size(px(28.0))
                .ml_auto()
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(remove_tip.clone()).build(window, cx))
                .when(!busy, |el| {
                    el.on_click(cx.listener(move |this, _, window, cx| {
                        this.set_wgsl(replacing, String::new(), window, cx);
                        this.fx_draft(replacing).shader_open = false;
                        cx.notify();
                    }))
                });
        let note = note.map(|(text, bad, retry)| {
            let color = if bad { p.destructive } else { rgb(0x059669) };
            let primary = p.primary;
            motion::slide_in(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .text_color(color)
                    .child(icon(if bad { "triangle-alert" } else { "check" }).size(px(14.0)))
                    .child(text.clone())
                    .when_some(id.clone().filter(|_| retry), |el, id| {
                        el.child(
                            div()
                                .id("pi-shader-retry")
                                .font_weight(FontWeight::BOLD)
                                .text_color(primary)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    status::retry(&id);
                                    cx.notify();
                                }))
                                .child(t("appsettings.shader.retry")),
                        )
                    }),
                SharedString::from(format!("pi-shader-note-{which}-{text}")),
                -6.0,
            )
            .into_any_element()
        });
        motion::slide_in(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_xs()
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.profileItems.shader")))
                        .child(
                            div()
                                .min_w_0()
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.profileItems.addShaderHint")),
                        )
                        .child(remove),
                )
                .child(starters)
                .child(
                    div()
                        .id(SharedString::from(format!("pi-wgsl-{which}")))
                        .cursor_text()
                        .on_click({
                            let state = state.clone();
                            move |_, window, cx| state.update(cx, |s, cx| s.focus(window, cx))
                        })
                        .min_h(px(144.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .bg(p.background)
                        .px(px(12.0))
                        .py(px(8.0))
                        .text_xs()
                        .font_family(crate::ui::theme::mono())
                        .shadow(shadow_sm())
                        .child(
                            Textarea::new(&state)
                                .appearance(false)
                                .with_size(px(13.72))
                                .font_family(crate::ui::theme::mono()),
                        ),
                )
                .children(note),
            SharedString::from(format!("pi-shader-{which}")),
            -6.0,
        )
        .into_any_element()
    }

    /// The spec box, the file button and what's wrong with it, the effect's
    /// pictures and shader (where the instance has them), beside a card playing it.
    fn effect_source(
        &mut self,
        replacing: bool,
        me: &pb::User,
        checked: &Result<Spec, crate::core::profile_items::EffectProblem>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = if replacing { self.replace.clone() } else { self.spec.clone() };
        let text = state.read(cx).value().to_string();
        let media = self.media_here();
        let extras: Vec<AnyElement> = if media {
            vec![self.effect_pictures(replacing, p, cx), self.shader_field(replacing, p, cx)]
        } else {
            Vec::new()
        };
        let status: Option<AnyElement> = match checked {
            Err(problem) if !text.trim().is_empty() => Some(
                motion::slide_in(
                    div().text_xs().text_color(p.destructive).child(t(problem.key())),
                    SharedString::from(format!("pi-problem-{problem:?}")),
                    -6.0,
                )
                .into_any_element(),
            ),
            Ok(_) => Some(
                motion::slide_in(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_xs()
                        .text_color(rgb(0x059669))
                        .child(icon("check").size(px(14.0)))
                        .child(t("serversettings.profileItems.effectOk")),
                    "pi-ok",
                    -6.0,
                )
                .into_any_element(),
            ),
            Err(_) => None,
        };
        let open = button(
            if replacing { "pi-open-replace" } else { "pi-open" },
            t("serversettings.profileItems.openFile"),
            Some("file-braces"),
            Look::Outline,
            true,
            p,
        )
        .rounded_full()
        .on_click(cx.listener(move |this, _, window, cx| this.open_spec(replacing, window, cx)));
        let card_w = 128.0;
        let mut card = mini_card(&me.id, -1, card_w, p);
        match checked {
            Ok(spec) => {
                let place = if replacing { "pi-replace-preview" } else { "pi-new-preview" };
                card = card.child(effect_in(
                    &mut self.effects,
                    place,
                    spec,
                    &me.id,
                    -1,
                    (card_w, card_w * 1.25),
                    true,
                    cx,
                ));
            }
            Err(_) => {
                card = card.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .top(px(card_w * 1.25 * 0.28))
                        .rounded_b(radius_xl())
                        .bg(alpha(p.card, 0.7))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .child(icon("wand").size(px(20.0))),
                );
            }
        }
        div()
            .flex()
            .gap(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id(if replacing { "pi-spec-replace" } else { "pi-spec" })
                            .cursor_text()
                            .on_click({
                                let state = state.clone();
                                move |_, window, cx| state.update(cx, |s, cx| s.focus(window, cx))
                            })
                            .min_h(px(144.0))
                            .rounded(radius_xl())
                            .border_1()
                            .border_color(p.border)
                            .bg(p.background)
                            .px(px(12.0))
                            .py(px(8.0))
                            .text_xs()
                            .font_family(crate::ui::theme::mono())
                            .shadow(shadow_sm())
                            .child(
                                Textarea::new(&state)
                                    .appearance(false)
                                    .with_size(px(13.72))
                                    .font_family(crate::ui::theme::mono()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(open)
                            .child(self.ask_ai(
                                if replacing { "pi-ask-replace" } else { "pi-ask-new" },
                                EFFECT_PROMPT,
                                t("serversettings.profileItems.askAiEffectHint"),
                                p,
                                cx,
                            ))
                            .child(Self::guide(
                                if replacing { "pi-guide-replace" } else { "pi-guide-new" },
                                EFFECT_DOCS,
                                p,
                            ))
                            .children(status),
                    )
                    .children(extras),
            )
            .child(div().flex_none().w(px(card_w)).child(card))
            .into_any_element()
    }

    fn draft_panel(&mut self, me: &pb::User, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let named = !self.name.read(cx).value().trim().is_empty();
        match self.draft.as_ref()? {
            Draft::Decoration { path } => {
                let local = decorated(avatar(Some(me), 80.0, p), 80.0, None)
                    .child(
                        img(path.clone())
                            .id("decoration-draft")
                            .absolute()
                            .left(px(-8.0))
                            .top(px(-8.0))
                            .size(px(96.0))
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element();
                let body = vec![
                    div()
                        .flex()
                        .items_center()
                        .gap(px(20.0))
                        .px(px(8.0))
                        .child(local)
                        .child(self.name_fields(p))
                        .into_any_element(),
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(t("serversettings.profileItems.decorationTip"))
                        .into_any_element(),
                ];
                Some(self.panel(
                    "pi-new-decoration",
                    t("serversettings.profileItems.newDecoration"),
                    body,
                    named,
                    t("serversettings.profileItems.add"),
                    p,
                    |this, cx| this.add(cx),
                    |this, cx| {
                        this.draft = None;
                        this.error = None;
                        cx.notify();
                    },
                    cx,
                ))
            }
            Draft::Nameplate { path } => {
                let local =
                    div()
                        .id("nameplate-draft-row")
                        .relative()
                        .w(px(256.0))
                        .h(px(48.0))
                        .flex_none()
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .bg(p.card)
                        .overflow_hidden()
                        .child(
                            div().absolute().top_0().bottom_0().right_0().w(px(288.0)).opacity(0.8).child(
                                img(path.clone()).id("nameplate-draft").size_full().object_fit(ObjectFit::Cover),
                            ),
                        )
                        // The fade the member list draws (`widgets::nameplate`), over the card.
                        .children([(0.0, 0.25, 1.0, 0.85), (0.25, 0.35, 0.85, 0.4), (0.6, 0.4, 0.4, 0.0)].map(
                            |(at, w, from, to)| {
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(gpui_kit::relative(at))
                                    .w(gpui_kit::relative(w))
                                    .bg(gpui_kit::linear_gradient(
                                        90.0,
                                        gpui_kit::linear_color_stop(alpha(p.card, from), 0.0),
                                        gpui_kit::linear_color_stop(alpha(p.card, to), 1.0),
                                    ))
                            },
                        ))
                        .child(avatar(Some(me), 32.0, p))
                        .child(
                            div()
                                .relative()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(crate::core::store::user_name(me)),
                        )
                        .into_any_element();
                let body = vec![
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(20.0))
                        .px(px(8.0))
                        .child(local)
                        .child(div().flex_1().min_w(px(200.0)).child(self.name_fields(p)))
                        .into_any_element(),
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(t("serversettings.profileItems.decorationTip"))
                        .into_any_element(),
                ];
                Some(self.panel(
                    "pi-new-nameplate",
                    t("serversettings.profileItems.newNameplate"),
                    body,
                    named,
                    t("serversettings.profileItems.add"),
                    p,
                    |this, cx| this.add(cx),
                    |this, cx| {
                        this.draft = None;
                        this.error = None;
                        cx.notify();
                    },
                    cx,
                ))
            }
            Draft::Effect => {
                let checked = self.checked(false, cx);
                let ok = checked.is_ok();
                let body =
                    vec![self.starters(p, cx), self.effect_source(false, me, &checked, p, cx), self.name_fields(p)];
                Some(self.panel(
                    "pi-new-effect",
                    t("serversettings.profileItems.newEffect"),
                    body,
                    ok && named,
                    t("serversettings.profileItems.add"),
                    p,
                    |this, cx| this.add(cx),
                    |this, cx| {
                        this.draft = None;
                        this.error = None;
                        cx.notify();
                    },
                    cx,
                ))
            }
        }
    }

    fn section(&self, title: String, count: usize, empty: String, rows: Vec<AnyElement>, p: &Palette) -> AnyElement {
        let mut grid = div().flex().flex_col().gap(px(8.0));
        let mut rows = rows.into_iter().peekable();
        while rows.peek().is_some() {
            let first = rows.next().expect("peeked");
            let second = rows.next();
            grid = grid.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w_0().child(first))
                    .child(div().flex_1().min_w_0().children(second)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(title.to_uppercase(), WIDE))
                    .child(
                        div()
                            .rounded_full()
                            .bg(p.muted)
                            .px(px(8.0))
                            .py(px(2.0))
                            .child(tracked(count.to_string(), WIDE)),
                    ),
            )
            .when(count == 0, |el| el.child(div().text_sm().text_color(p.muted_foreground).child(empty)))
            .child(grid)
            .into_any_element()
    }

    fn row(&mut self, item: &pb::ProfileItem, me: &pb::User, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id.clone();
        let decoration = item.kind == pb::ProfileItemKind::Decoration as i32;
        let plate = item.kind == pb::ProfileItemKind::Nameplate as i32;
        let spec = (!decoration && !plate).then(|| item_effect(item)).flatten();
        let lively = self.hovered.as_deref() == Some(id.as_str());
        let face: AnyElement = if plate {
            let group = SharedString::from(format!("pi-row-{id}"));
            div()
                .id(SharedString::from(format!("pi-plate-{id}")))
                .group(group.clone())
                .relative()
                .w(px(112.0))
                .h(px(44.0))
                .flex_none()
                .px(px(8.0))
                .flex()
                .items_center()
                .rounded(radius_xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .child(crate::ui::widgets::nameplate(&item.picture_url, 44.0, radius_xl(), p.card, p.card, group))
                .child(avatar(Some(me), 28.0, p))
                .into_any_element()
        } else if decoration {
            div()
                .size(px(64.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(decorated(avatar(Some(me), 48.0, p), 48.0, Some(&item.picture_url)))
                .into_any_element()
        } else {
            let mut card = mini_card(&me.id, -1, 56.0, p);
            if let Some(spec) = &spec {
                card = card.child(effect_in(
                    &mut self.effects,
                    &format!("pi-row-{id}"),
                    spec,
                    &me.id,
                    -1,
                    (56.0, 70.0),
                    lively,
                    cx,
                ));
            }
            div().flex_none().w(px(56.0)).child(card).into_any_element()
        };
        let meta = if decoration || plate {
            let size = crate::core::attachments::format_bytes(item.size);
            if item.animated { format!("{size} · {}", t("serversettings.emoji.moves")) } else { size }
        } else if let Some(spec) = &spec {
            let mut parts = Vec::new();
            if !item.pictures.is_empty() {
                let count = t_with(
                    "serversettings.profileItems.pictureCount",
                    &[("count", Arg::Num(item.pictures.len() as i64))],
                );
                parts.push(format!("{count} ({})", crate::core::attachments::format_bytes(item.size)));
            }
            if spec.shader.is_some() {
                parts.push(t("serversettings.profileItems.shader"));
            }
            parts.join(" · ")
        } else {
            t("serversettings.profileItems.cantPlay")
        };
        let inline = self.inline.get(&id).expect("filled before drawing");
        let shake_at = inline.shook.filter(|at| at.elapsed() < Duration::from_millis(400));
        let name_field = div()
            .h(px(28.0))
            .min_w_0()
            .flex()
            .items_center()
            .font_weight(FontWeight::BOLD)
            .ml(px(-5.0))
            .child(div().flex_1().min_w_0().child(Input::new(&inline.name).appearance(false).with_size(px(16.0))))
            .when(inline.saving, |el| {
                el.child(icon("loader-circle").size(px(14.0)).text_color(p.muted_foreground).ml(px(4.0)))
            });
        let name_field: AnyElement = match shake_at {
            Some(at) => name_field
                .with_animation(
                    SharedString::from(format!("pi-shake-{id}-{at:?}")),
                    gpui_kit::Animation::new(Duration::from_millis(350)),
                    |el, t| el.relative().left(px(crate::ui::instance_home::shake(t))),
                )
                .into_any_element(),
            None => name_field.into_any_element(),
        };
        let description_field = div()
            .h(px(28.0))
            .min_w_0()
            .ml(px(-5.0))
            .flex()
            .items_center()
            .text_color(p.muted_foreground)
            .child(Input::new(&inline.description).appearance(false).with_size(px(16.0)));
        let confirming = self.confirming.as_deref() == Some(id.as_str());
        let replacing = self.replacing.as_deref() == Some(id.as_str());
        let actions: AnyElement = if confirming {
            div()
                .flex()
                .gap(px(4.0))
                .child(
                    button(
                        SharedString::from(format!("pi-delete-yes|{id}")),
                        t("serversettings.shared.delete"),
                        None,
                        Look::Destructive,
                        true,
                        p,
                    )
                    .rounded_full()
                    .px(px(12.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| this.delete(id.clone(), cx))
                    }),
                )
                .child(round_icon(SharedString::from(format!("pi-keep|{id}")), "x", p, p.foreground.into()).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.confirming = None;
                        cx.notify();
                    }),
                ))
                .into_any_element()
        } else {
            let item_c = item.clone();
            div()
                .id("pi-actions")
                .flex()
                .gap(px(4.0))
                .opacity(0.6)
                .group_hover("pi-row", |s| s.opacity(1.0))
                .when(!decoration, |el| {
                    el.child(
                        round_icon(SharedString::from(format!("pi-replace|{id}")), "pencil", p, p.foreground.into())
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.start_replacing(&item_c, window, cx)),
                            ),
                    )
                })
                .child(
                    round_icon(SharedString::from(format!("pi-delete|{id}")), "trash", p, p.destructive.into())
                        .on_click({
                            let id = id.clone();
                            cx.listener(move |this, _, _, cx| {
                                this.confirming = Some(id.clone());
                                cx.notify();
                            })
                        }),
                )
                .into_any_element()
        };
        let replace = replacing.then(|| {
            let checked = self.checked(true, cx);
            let ok = checked.is_ok() && self.replace_changed(cx);
            let body = vec![self.effect_source(true, me, &checked, p, cx)];
            let item = item.clone();
            self.panel(
                "pi-replace",
                t("serversettings.profileItems.replace"),
                body,
                ok,
                t("serversettings.profileItems.add"),
                p,
                move |this, cx| this.save_replace(item.clone(), cx),
                |this, cx| {
                    this.replacing = None;
                    this.error = None;
                    cx.notify();
                },
                cx,
            )
        });
        let hover_border = alpha(p.primary, 0.3);
        let hover_id = id.clone();
        motion::rise(
            div()
                .id(SharedString::from(format!("pi|{id}")))
                .group("pi-row")
                .flex()
                .flex_col()
                .gap(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.5))
                .p(px(12.0))
                .hover(move |s| s.border_color(hover_border))
                .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                    if *on {
                        this.hovered = Some(hover_id.clone());
                    } else if this.hovered.as_deref() == Some(hover_id.as_str()) {
                        this.hovered = None;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(face)
                        .child(
                            div().flex_1().min_w_0().flex().flex_col().child(name_field).child(description_field).when(
                                !meta.is_empty(),
                                |el| {
                                    el.child(
                                        div()
                                            .px(px(2.0))
                                            .truncate()
                                            .text_size(px(11.2))
                                            .line_height(px(14.0))
                                            .text_color(p.muted_foreground)
                                            .child(meta),
                                    )
                                },
                            ),
                        )
                        .child(actions),
                )
                .when(confirming, |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.profileItems.deleteWarn")),
                    )
                })
                .children(replace),
            SharedString::from(format!("pi-in|{id}")),
            Duration::ZERO,
            6.0,
        )
        .into_any_element()
    }
}

/// A round ghost button holding a glyph (`size="icon"`, `size-8 rounded-full`), muted until
/// hovered, then `hover` colored.
fn round_icon(
    id: impl Into<gpui_kit::ElementId>,
    glyph: &str,
    p: &Palette,
    hover: gpui_kit::Hsla,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let bg = p.accent;
    div()
        .id(id)
        .size(px(32.0))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(p.muted_foreground)
        .cursor_pointer()
        .hover(move |s| s.bg(bg).text_color(hover))
        .active(|s| s.translate_y(px(1.0)))
        .child(icon(glyph).size(px(16.0)))
}

/// A draft's pictures after they were uploaded: each file becomes its link,
/// so trying again doesn't upload it again.
fn uploaded(draft: &mut EffectDraft, urls: Vec<String>) {
    if urls.len() != draft.pictures.len() {
        return;
    }
    for (picture, url) in draft.pictures.iter_mut().zip(urls) {
        if let DraftPicture::File(path) = picture {
            let animated = path.to_string_lossy().to_ascii_lowercase().ends_with(".gif");
            *picture = DraftPicture::Uploaded { url, animated };
        }
    }
}

/// A spec laid out to read and edit, or as it is if it isn't JSON.
fn pretty(json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| json.to_owned())
}

/// An effect's spec from its file, as text: the box checks the rest. Small enough to read here.
fn read_effect(path: &std::path::Path) -> Result<String, Problem> {
    let unreadable = || Problem::new(tonic::Code::NotFound, t("desktop.look.cantRead"));
    let size = std::fs::metadata(path).map_err(|_| unreadable())?.len();
    if size > MOST_EFFECT {
        return Err(Problem::new(tonic::Code::InvalidArgument, t("desktop.profileItems.effectTooBig")));
    }
    std::fs::read_to_string(path).map_err(|_| unreadable())
}

impl Render for ProfileItemsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let Some(me) = self.me() else { return div().into_any_element() };
        let items = self.items();
        for item in &items {
            self.inline_for(item, window, cx);
        }
        let intro = t(if self.scope == Scope::Instance {
            "serversettings.profileItems.introInstance"
        } else {
            "serversettings.profileItems.introServer"
        });
        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(div().text_sm().text_color(p.muted_foreground).child(intro))
            .child(self.adders(&p, cx));
        match self.draft_panel(&me, &p, cx) {
            Some(panel) => page = page.child(panel),
            None => {
                if let Some(error) = self.error.clone().filter(|_| self.replacing.is_none()) {
                    page = page.child(div().text_sm().text_color(p.destructive).child(error));
                }
            }
        }
        if !self.loaded {
            let shimmer = |n: usize| crate::ui::instance_home::shimmer(80.0, radius_2xl(), &p, window, n);
            return page
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(div().flex_1().child(shimmer(0)))
                                .child(div().flex_1().child(shimmer(1))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(div().flex_1().child(shimmer(2)))
                                .child(div().flex_1().child(shimmer(3))),
                        ),
                )
                .into_any_element();
        }
        let (decorations, effects): (Vec<_>, Vec<_>) =
            items.iter().partition(|i| i.kind == pb::ProfileItemKind::Decoration as i32);
        let nameplates: Vec<&pb::ProfileItem> =
            effects.iter().copied().filter(|i| i.kind == pb::ProfileItemKind::Nameplate as i32).collect();
        let effects: Vec<&pb::ProfileItem> =
            effects.into_iter().filter(|i| i.kind == pb::ProfileItemKind::Effect as i32).collect();
        let decoration_rows: Vec<AnyElement> = decorations.iter().map(|i| self.row(i, &me, &p, cx)).collect();
        let nameplate_rows: Vec<AnyElement> = nameplates.iter().map(|i| self.row(i, &me, &p, cx)).collect();
        let nameplates_here = self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| i.has("nameplates")));
        let effect_rows: Vec<AnyElement> = effects.iter().map(|i| self.row(i, &me, &p, cx)).collect();
        // An effect being replaced spans both columns: it goes on a line of its own.
        let effect_section =
            if let Some(n) = self.replacing.as_ref().and_then(|id| effects.iter().position(|i| &i.id == id)) {
                let mut rows = effect_rows;
                let wide = rows.remove(n);
                let (before, after): (Vec<_>, Vec<_>) = rows.into_iter().enumerate().partition(|(k, _)| *k < n);
                let section = self.section(
                    t("serversettings.profileItems.effects"),
                    effects.len(),
                    t("serversettings.profileItems.noEffects"),
                    before.into_iter().map(|(_, r)| r).collect(),
                    &p,
                );
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(section)
                    .child(wide)
                    .child(self.section_rows(after.into_iter().map(|(_, r)| r).collect()))
                    .into_any_element()
            } else {
                self.section(
                    t("serversettings.profileItems.effects"),
                    effects.len(),
                    t("serversettings.profileItems.noEffects"),
                    effect_rows,
                    &p,
                )
            };
        page.child(self.section(
            t("serversettings.profileItems.decorations"),
            decorations.len(),
            t("serversettings.profileItems.noDecorations"),
            decoration_rows,
            &p,
        ))
        .when(nameplates_here || !nameplates.is_empty(), |el| {
            el.child(self.section(
                t("serversettings.profileItems.nameplates"),
                nameplates.len(),
                t("serversettings.profileItems.noNameplates"),
                nameplate_rows,
                &p,
            ))
        })
        .child(effect_section)
        .into_any_element()
    }
}

impl ProfileItemsView {
    /// Rows two to a line, without a heading.
    fn section_rows(&self, rows: Vec<AnyElement>) -> AnyElement {
        let mut grid = div().flex().flex_col().gap(px(8.0));
        let mut rows = rows.into_iter().peekable();
        while rows.peek().is_some() {
            let first = rows.next().expect("peeked");
            let second = rows.next();
            grid = grid.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w_0().child(first))
                    .child(div().flex_1().min_w_0().children(second)),
            );
        }
        grid.into_any_element()
    }
}
