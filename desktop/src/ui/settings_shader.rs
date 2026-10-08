//! Writing a custom shader, as the web's `settings/app/ShaderEditor.tsx`:
//! its name, the starters, the WGSL (checked by the GPU's own compiler once
//! typing settles), what it can read, and what shows where it can't run.
//! Code that compiles goes straight to the backdrop; code that doesn't stays
//! a draft here, with the error at its line, so the backdrop never shows a
//! broken shader.

use std::time::Duration;

use gpui_kit::base::input::{Diagnostic as Mark, DiagnosticSeverity, Position};
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState, TabSize};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::effects::custom::{CustomShader, FALLBACKS, MAX_SHADER_BYTES, SHADER_NAME_MAX, STARTERS, shader_id};
use crate::core::effects::gpu::{self, Diagnostic, Gpu};
use crate::core::effects::status::{self, ShaderStatus};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::themes::Effect;
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_themes::Target;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_lg, radius_xl};
use crate::ui::widgets::icon;

/// How long typing has to pause before the shader is compiled and tried.
const SETTLE: Duration = Duration::from_millis(450);

/// The inputs, as the docs list them (docs/themes.md, Custom shaders).
const INPUTS: [(&str, &str); 10] = [
    ("uv", "appsettings.shader.input.uv"),
    ("fuwa.time", "appsettings.shader.input.time"),
    ("fuwa.res", "appsettings.shader.input.res"),
    ("fuwa.pointer", "appsettings.shader.input.pointer"),
    ("fuwa.primary", "appsettings.shader.input.primary"),
    ("fuwa.accent", "appsettings.shader.input.accent"),
    ("fuwa.background", "appsettings.shader.input.background"),
    ("fuwa.foreground", "appsettings.shader.input.foreground"),
    ("fuwa.intensity", "appsettings.shader.input.intensity"),
    ("hash21(p) noise(p) fbm(p)", "appsettings.shader.input.noise"),
];

/// The editor's own state.
#[derive(Default)]
pub(crate) struct ShaderForm {
    /// Which backdrop it edits.
    target: Option<Target>,
    name: Option<Entity<InputState>>,
    code: Option<Entity<EditorState>>,
    /// The shader's code last put in the editor, to notice one set from elsewhere (a starter, an import).
    seen: String,
    /// What's typed.
    draft: String,
    /// The last draft the compiler looked at, and what it said.
    checked: (String, Vec<Diagnostic>),
    /// Counts edits, so only the latest one's check counts.
    edits: u64,
    reference: bool,
}

/// The badge's look.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Ok,
    Wait,
    Bad,
}

/// Where a shader stands: its icon, words and tone, and whether it can be tried again.
fn describe(
    status: Option<ShaderStatus>,
    checking: bool,
    broken: bool,
    fallback: Effect,
) -> (&'static str, String, Tone, bool) {
    let instead = if fallback == Effect::None {
        t("appsettings.shader.nothing")
    } else {
        t(&format!("appsettings.backdrop.effect.{}", fallback.key()))
    };
    if checking {
        return ("loader-circle", t("appsettings.shader.compiling"), Tone::Wait, false);
    }
    if broken {
        return ("triangle-alert", t("appsettings.shader.doesNotCompile"), Tone::Bad, false);
    }
    if gpu::gpu() == Gpu::Missing && !matches!(status, Some(ShaderStatus::Slow { .. } | ShaderStatus::Stopped)) {
        return ("cpu", t_with("desktop.shader.noGpu", &[("fallback", Arg::Str(&instead))]), Tone::Wait, false);
    }
    match status {
        None => ("loader-circle", t("appsettings.shader.trying"), Tone::Wait, false),
        Some(ShaderStatus::Running { scale, .. }) => {
            let key = if scale >= 0.75 {
                "appsettings.shader.running"
            } else if scale >= 0.5 {
                "appsettings.shader.runningHalf"
            } else {
                "appsettings.shader.runningThird"
            };
            ("sparkles", t(key), Tone::Ok, false)
        }
        Some(ShaderStatus::Slow { .. }) => {
            ("turtle", t_with("appsettings.shader.slow", &[("fallback", Arg::Str(&instead))]), Tone::Bad, true)
        }
        Some(ShaderStatus::Stopped) => {
            ("zap-off", t_with("appsettings.shader.stopped", &[("fallback", Arg::Str(&instead))]), Tone::Bad, true)
        }
        Some(ShaderStatus::Broken { message, line }) => {
            let text = match line {
                Some(line) => {
                    format!("{} {message}", t_with("appsettings.shader.line", &[("line", Arg::Num(line as i64))]))
                }
                None => message,
            };
            ("triangle-alert", text, Tone::Bad, true)
        }
    }
}

impl SettingsView {
    /// Changes the shader of a backdrop.
    fn patch_shader(&mut self, target: Target, cx: &mut Context<Self>, f: impl FnOnce(&mut CustomShader)) {
        self.patch_backdrop(target, cx, |b| {
            if let Some(shader) = b.shader.as_mut() {
                f(shader);
            }
        });
    }

    /// The editor's inputs, made when it opens (or moves to another backdrop) and
    /// given a shader set from elsewhere.
    fn shader_inputs(&mut self, target: Target, shader: &CustomShader, window: &mut Window, cx: &mut Context<Self>) {
        let form = &mut self.themes.shader;
        if form.target != Some(target) || form.code.is_none() {
            let name = cx.new(|cx| InputState::new(window, cx));
            name.update(cx, |s, cx| s.set_value(shader.name.clone(), window, cx));
            cx.subscribe(&name, move |this: &mut SettingsView, s, e: &InputEvent, cx| {
                let value: String = s.read(cx).value().chars().take(SHADER_NAME_MAX).collect();
                match e {
                    InputEvent::Change if !value.trim().is_empty() => {
                        this.patch_shader(target, cx, |sh| sh.name = value.trim().to_owned())
                    }
                    // Left empty, it's called what new shaders are.
                    InputEvent::Blur if value.trim().is_empty() => {
                        this.patch_shader(target, cx, |sh| sh.name = t("appsettings.shader.defaultName"));
                        this.themes.shader.name = None;
                        this.themes.shader.target = None;
                    }
                    _ => {}
                }
            })
            .detach();
            let code = cx.new(|cx| {
                EditorState::new(window, cx)
                    .line_number(true)
                    .tab_size(TabSize { tab_size: 2, hard_tabs: false })
                    .searchable(false)
                    .soft_wrap(false)
            });
            code.update(cx, |s, cx| s.set_value(shader.code.clone(), window, cx));
            cx.subscribe(&code, move |this: &mut SettingsView, s, e: &InputEvent, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                let draft = s.read(cx).value().to_string();
                if draft == this.themes.shader.draft {
                    return;
                }
                this.themes.shader.draft = draft.clone();
                this.themes.shader.edits += 1;
                let edit = this.themes.shader.edits;
                cx.notify();
                // Checked once typing settles; committed if it's sound.
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SETTLE).await;
                    if this.update(cx, |this, _| this.themes.shader.edits != edit).unwrap_or(true) {
                        return;
                    }
                    let found = gpu::check(draft.clone()).await.ok().flatten().unwrap_or_default();
                    let _ = this.update(cx, |this, cx| {
                        if this.themes.shader.edits != edit {
                            return;
                        }
                        this.themes.shader.checked = (draft.clone(), found.clone());
                        if found.is_empty() {
                            this.themes.shader.seen = draft.clone();
                            this.patch_shader(target, cx, |sh| sh.code = draft);
                        }
                        cx.notify();
                    });
                })
                .detach();
            })
            .detach();
            *form = ShaderForm {
                target: Some(target),
                name: Some(name),
                code: Some(code),
                seen: shader.code.clone(),
                draft: shader.code.clone(),
                checked: (shader.code.clone(), Vec::new()),
                edits: 0,
                reference: form.reference,
            };
            return;
        }
        // A shader set from elsewhere (a starter, an imported theme, Reset) replaces the draft.
        if form.seen != shader.code {
            form.seen = shader.code.clone();
            form.draft = shader.code.clone();
            form.checked = (shader.code.clone(), Vec::new());
            form.edits += 1;
            if let Some(code) = form.code.clone() {
                code.update(cx, |s, cx| s.set_value(shader.code.clone(), window, cx));
            }
        }
    }

    /// The editor for a backdrop's custom shader.
    pub(crate) fn shader_editor(
        &mut self,
        target: Target,
        shader: &CustomShader,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.shader_inputs(target, shader, window, cx);
        let form = &self.themes.shader;
        let (Some(name), Some(code)) = (form.name.clone(), form.code.clone()) else {
            return div().into_any_element();
        };
        let dirty = form.draft != shader.code;
        let checking = dirty && form.checked.0 != form.draft;
        let errors: Vec<Diagnostic> = if dirty && !checking { form.checked.1.clone() } else { Vec::new() };
        let size = form.draft.len();
        let reference = form.reference;
        let edits = form.edits;
        // The lines with errors, marked in the editor.
        code.update(cx, |s, cx| {
            let text = s.text().clone();
            if let Some(marks) = s.diagnostics_mut() {
                marks.reset(&text);
                for e in &errors {
                    if let Some(line) = e.line {
                        let line = (line - 1) as u32;
                        let at = e.column.map_or(0, |c| c.saturating_sub(1) as u32);
                        marks.push(
                            Mark::new(Position::new(line, at)..Position::new(line, at + 1), e.message.clone())
                                .with_severity(DiagnosticSeverity::Error),
                        );
                    }
                }
            }
            cx.notify();
        });

        let id = shader_id(&shader.code);
        let (glyph, words, tone, retry) = describe(status::status(&id), checking, !errors.is_empty(), shader.fallback);
        let (bg, fg): (Hsla, Hsla) = match tone {
            Tone::Ok => (alpha(p.primary, 0.15), p.primary.into()),
            Tone::Wait => (p.muted.into(), p.muted_foreground.into()),
            Tone::Bad => (alpha(p.destructive, 0.12), p.destructive.into()),
        };
        let spins = glyph == "loader-circle";
        let badge = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(motion::rise(
                div()
                    .max_w(px(256.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded_full()
                    .px(px(8.0))
                    .py(px(2.0))
                    .bg(bg)
                    .text_color(fg)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(if spins {
                        motion::ambient(
                            icon(glyph).size(px(12.0)),
                            "shader-spin",
                            Duration::from_millis(900),
                            window,
                            |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                        )
                    } else {
                        icon(glyph).size(px(12.0)).into_any_element()
                    })
                    .child(div().truncate().child(words)),
                SharedString::from(format!("shader-badge-{glyph}-{}", tone as u8)),
                Duration::ZERO,
                6.0,
            ))
            .when(retry, |el| {
                let id = id.clone();
                el.child(
                    div()
                        .id("shader-retry")
                        .size(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(|s| s.bg(p.muted).text_color(p.foreground))
                        .active(|s| s.top(px(1.0)))
                        .tooltip(move |window, cx| {
                            crate::ui::overlay::Tip::new(t("appsettings.shader.retry")).build(window, cx)
                        })
                        .on_click(cx.listener(move |_, _, _, cx| {
                            status::retry(&id);
                            cx.notify();
                        }))
                        .child(icon("rotate-ccw").size(px(14.0))),
                )
            });

        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(120.0))
                    .text_sm()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(Input::new(&name).appearance(false)),
            )
            .child(badge);

        let label = |key: &str| {
            div()
                .mr(px(4.0))
                .text_size(px(10.4))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(t(key).to_uppercase())
        };
        let mut starters =
            div().flex().flex_wrap().items_center().gap(px(6.0)).child(label("appsettings.shader.startFrom"));
        for (n, starter) in STARTERS.iter().enumerate() {
            let on = shader.code == starter.code;
            let hover = alpha(p.primary, 0.5);
            let (primary, hint) = (p.primary, starter.hint);
            starters = starters.child(motion::rise(
                div()
                    .id(SharedString::from(format!("shader-starter-{}", starter.id)))
                    .rounded_full()
                    .border_1()
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .map(|el| {
                        if on {
                            el.border_color(p.primary)
                                .bg(alpha(p.primary, 0.1))
                                .text_color(p.primary)
                                .hover(|s| s.top(px(-2.0)))
                        } else {
                            el.border_color(p.border)
                                .hover(move |s| s.border_color(hover).text_color(primary).top(px(-2.0)))
                        }
                    })
                    .active(|s| s.top(px(1.0)))
                    .tooltip(move |window, cx| crate::ui::overlay::Tip::new(t(hint)).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let fallback = this.backdrop_shader(target).map_or(Effect::Aurora, |s| s.fallback);
                        this.patch_backdrop(target, cx, |b| {
                            b.shader = Some(CustomShader { fallback, ..STARTERS[n].shader() });
                        });
                    }))
                    .child(starter.name),
                SharedString::from(format!("shader-starter-in-{n}")),
                Duration::from_millis(40 * n as u64),
                4.0,
            ));
        }

        let lines: Vec<usize> = errors.iter().filter_map(|e| e.line).collect();
        let area = div()
            .h(px(352.0))
            .overflow_hidden()
            .rounded(radius_xl())
            .border_1()
            .border_color(if lines.is_empty() { p.border.into() } else { alpha(p.destructive, 0.5) })
            .bg(alpha(p.background, 0.8))
            .font_family("monospace")
            .text_size(px(12.5))
            .child(Editor::new(&code).appearance(false).h(px(352.0)));

        let error_list = (!errors.is_empty()).then(|| {
            let mut list = div().flex().flex_col().gap(px(4.0)).text_xs();
            for e in errors.iter().take(4) {
                list =
                    list.child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .rounded(radius_lg())
                            .bg(alpha(p.destructive, 0.1))
                            .px(px(8.0))
                            .py(px(6.0))
                            .text_color(p.destructive)
                            .child(icon("triangle-alert").size(px(14.0)).flex_none().mt(px(1.0)))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_wrap()
                                    .when_some(e.line, |el, line| {
                                        el.child(div().font_weight(FontWeight::BOLD).mr(px(4.0)).child(t_with(
                                            "appsettings.shader.line",
                                            &[("line", Arg::Num(line as i64))],
                                        )))
                                    })
                                    .child(e.message.clone()),
                            ),
                    );
            }
            list =
                list.child(div().px(px(4.0)).text_color(p.muted_foreground).child(t("appsettings.shader.keepsLast")));
            motion::rise(list, SharedString::from(format!("shader-errors-{edits}")), Duration::ZERO, -6.0)
        });

        let too_big = size > MAX_SHADER_BYTES;
        let foot = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .text_xs()
            .child(
                div()
                    .id("shader-reference")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.primary)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.themes.shader.reference = !this.themes.shader.reference;
                        cx.notify();
                    }))
                    .child(icon("book-open").size(px(14.0)))
                    .child(t("appsettings.shader.reference"))
                    .child(icon(if reference { "chevron-up" } else { "chevron-down" }).size(px(14.0))),
            )
            .child(
                div()
                    .text_color(if too_big { p.destructive } else { p.muted_foreground })
                    .when(too_big, |el| el.font_weight(FontWeight::BOLD))
                    .child(t_with(
                        "appsettings.shader.size",
                        &[
                            ("size", Arg::Str(&format!("{:.1}", size as f32 / 1024.0))),
                            ("max", Arg::Num((MAX_SHADER_BYTES / 1024) as i64)),
                        ],
                    )),
            );

        let reference = reference.then(|| {
            let mut rows = div().flex().flex_col().gap(px(4.0));
            for (input, hint) in INPUTS {
                rows = rows.child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(
                            div()
                                .w(px(176.0))
                                .flex_none()
                                .font_family("monospace")
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.primary)
                                .child(input),
                        )
                        .child(div().flex_1().text_color(p.muted_foreground).child(t(hint))),
                );
            }
            motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.muted, 0.5))
                    .p(px(12.0))
                    .text_xs()
                    .child(t_with(
                        "appsettings.shader.write",
                        &[("signature", Arg::Str("fn shade(uv: vec2f) -> vec4f"))],
                    ))
                    .child(rows)
                    .child(div().text_color(p.muted_foreground).child(t("appsettings.shader.limits"))),
                "shader-reference-in",
                Duration::ZERO,
                -6.0,
            )
        });

        let mut pills = div().flex().flex_wrap().gap(px(6.0));
        for fallback in FALLBACKS {
            let on = fallback == shader.fallback;
            let name = if fallback == Effect::None {
                t("appsettings.shader.nothing")
            } else {
                t(&format!("appsettings.backdrop.effect.{}", fallback.key()))
            };
            let fg = p.foreground;
            pills = pills.child(
                div()
                    .id(SharedString::from(format!("shader-fallback-{}", fallback.key())))
                    .rounded_full()
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .map(|el| {
                        if on {
                            el.bg(p.primary).text_color(p.primary_foreground)
                        } else {
                            el.text_color(p.muted_foreground).hover(move |s| s.text_color(fg))
                        }
                    })
                    .active(|s| s.top(px(1.0)))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.patch_shader(target, cx, |s| s.fallback = fallback)),
                    )
                    .child(name),
            );
        }
        let fallback = div().flex().flex_col().gap(px(6.0)).child(label("appsettings.shader.fallback")).child(pills);

        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .overflow_hidden()
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.card, 0.6))
                .p(px(12.0))
                .child(head)
                .child(starters)
                .child(area)
                .children(error_list)
                .child(foot)
                .children(reference)
                .child(fallback),
            "shader-editor-in",
            Duration::ZERO,
            -8.0,
        )
        .into_any_element()
    }

    /// The shader of a backdrop being edited.
    fn backdrop_shader(&self, target: Target) -> Option<CustomShader> {
        match target {
            Target::App => self.core.prefs().backdrop.shader,
            Target::Draft => {
                self.themes.draft.as_ref().and_then(|(t, _)| t.backdrop.as_ref()).and_then(|b| b.shader.clone())
            }
        }
    }
}
