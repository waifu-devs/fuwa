//! Rules & questions, the web's `JoinFormEditor.tsx`: the rules new members
//! agree to before they talk (dragged into order, with starters to begin
//! from) and the questions people answer when they apply (a line or a few
//! paragraphs, optional or not), beside a preview of both as people see them
//! on their way in.

use gpui_kit::Render;

use super::pages::{boxed, focused, shimmers};
use super::*;
use crate::core::server_pages::{MAX_QUESTIONS, MAX_RULES, PROMPT_MAX, RULE_MAX, cleaned_form, form_changes, ideas};
use crate::ui::settings_controls::{Look, button, segmented};
use crate::ui::theme::{radius_2xl, radius_lg, radius_xl};

/// The answer-length switches' ids, one per question.
const LENGTH_IDS: [&str; MAX_QUESTIONS] =
    ["answer-length-0", "answer-length-1", "answer-length-2", "answer-length-3", "answer-length-4"];

/// The rules to start from, as catalog keys.
const STARTERS: [&str; 3] = [
    "serversettings.joinForm.starterKind",
    "serversettings.joinForm.starterSafe",
    "serversettings.joinForm.starterSpam",
];

pub(super) struct Rule {
    id: u64,
    text: Entity<InputState>,
}

pub(super) struct Question {
    id: u64,
    prompt: Entity<InputState>,
    paragraph: bool,
    required: bool,
}

#[derive(Default)]
pub(super) struct JoinForm {
    saved: Option<pb::JoinForm>,
    rules: Vec<Rule>,
    questions: Vec<Question>,
    loading: bool,
    problem: Option<String>,
    saving: bool,
    error: Option<String>,
    /// The preview's "I agree", ticked for fun.
    agreed: bool,
    next: u64,
    /// Rows made by the last press, which take the keyboard once drawn.
    focus: Option<u64>,
    subscriptions: Vec<Subscription>,
}

impl JoinForm {
    /// The rules and questions as they're saved, once read.
    pub(super) fn saved_form(&self) -> Option<pb::JoinForm> {
        self.saved.clone()
    }
}

/// A rule held by its handle, following the pointer.
#[derive(Clone)]
pub(super) struct RuleDrag {
    id: u64,
    index: usize,
    text: String,
}

impl Render for RuleDrag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        div()
            .w(px(360.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .p(px(6.0))
            .pl(px(4.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(alpha(p.primary, 0.5))
            .bg(p.card)
            .text_sm()
            .shadow(vec![gpui_kit::BoxShadow {
                color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.35),
                offset: gpui_kit::point(px(0.0), px(12.0)),
                blur_radius: px(30.0),
                spread_radius: px(-12.0),
                inset: false,
            }])
            .child(
                div()
                    .w(px(24.0))
                    .flex()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .child(icon("grip-vertical").size(px(16.0))),
            )
            .child(number(self.index, &p))
            .child(div().flex_1().min_w_0().truncate().child(self.text.clone()))
    }
}

/// A rule's number in a soft circle.
fn number(n: usize, p: &Palette) -> gpui_kit::Div {
    div()
        .size(px(24.0))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(p.primary, 0.15))
        .text_color(p.primary)
        .text_xs()
        .font_weight(FontWeight::EXTRA_BOLD)
        .child((n + 1).to_string())
}

impl ServerSettingsView {
    fn new_rule(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> Rule {
        let f = &mut self.pages.join;
        f.next += 1;
        let id = f.next;
        let text = text.to_owned();
        let state = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(t("serversettings.joinForm.writeRule"));
            s.set_value(text, window, cx);
            s
        });
        let sub = cx.subscribe(&state, |_: &mut Self, _, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                cx.notify();
            }
        });
        self.pages.join.subscriptions.push(sub);
        Rule { id, text: state }
    }

    fn new_question(&mut self, q: &pb::JoinQuestion, window: &mut Window, cx: &mut Context<Self>) -> Question {
        let f = &mut self.pages.join;
        f.next += 1;
        let id = f.next;
        let prompt = q.prompt.clone();
        let state = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(t("serversettings.joinForm.questionPlaceholder"));
            s.set_value(prompt, window, cx);
            s
        });
        let sub = cx.subscribe(&state, |_: &mut Self, _, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                cx.notify();
            }
        });
        self.pages.join.subscriptions.push(sub);
        Question { id, prompt: state, paragraph: q.paragraph, required: q.required }
    }

    /// Fills the editor from a saved form.
    fn take_form(&mut self, form: pb::JoinForm, window: &mut Window, cx: &mut Context<Self>) {
        self.pages.join.subscriptions.clear();
        let rules: Vec<Rule> = form.rules.iter().map(|r| self.new_rule(r, window, cx)).collect();
        let questions: Vec<Question> = form.questions.iter().map(|q| self.new_question(q, window, cx)).collect();
        let f = &mut self.pages.join;
        f.rules = rules;
        f.questions = questions;
        f.saved = Some(form);
        f.focus = None;
    }

    pub(super) fn load_join_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let f = &mut self.pages.join;
        if f.loading || f.saved.is_some() || f.problem.is_some() {
            return;
        }
        f.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = self.core.spawn(async move { core.get_join_form(&key, &sid).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.pages.join.loading = false;
                match result {
                    Ok(form) => this.take_form(form, window, cx),
                    Err(err) => this.pages.join.problem = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The form as it would be saved now.
    fn form_draft(&self, cx: &Context<Self>) -> pb::JoinForm {
        let f = &self.pages.join;
        let rules: Vec<String> = f.rules.iter().map(|r| r.text.read(cx).value().to_string()).collect();
        let questions: Vec<pb::JoinQuestion> = f
            .questions
            .iter()
            .map(|q| pb::JoinQuestion {
                prompt: q.prompt.read(cx).value().to_string(),
                paragraph: q.paragraph,
                required: q.required,
            })
            .collect();
        cleaned_form(&rules, &questions)
    }

    fn save_join_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.form_draft(cx);
        self.pages.join.saving = true;
        self.pages.join.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = self.core.spawn(async move { core.set_join_form(&key, &sid, draft).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.pages.join.saving = false;
                match result {
                    Ok(form) => this.take_form(form, window, cx),
                    Err(err) => this.pages.join.error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn add_join_rule(&mut self, text: &str, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.join.rules.len() >= MAX_RULES {
            return;
        }
        let rule = self.new_rule(text, window, cx);
        if focus {
            self.pages.join.focus = Some(rule.id);
        }
        self.pages.join.rules.push(rule);
        cx.notify();
    }

    pub(super) fn join_form_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.load_join_form(window, cx);
        if let Some(problem) = &self.pages.join.problem {
            return super::pages::problem(problem, p);
        }
        let Some(saved) = self.pages.join.saved.clone() else {
            return shimmers(3, 56.0, radius_2xl(), p, window);
        };
        // A row just added takes the keyboard.
        if let Some(id) = self.pages.join.focus.take() {
            let f = &self.pages.join;
            let state = f
                .rules
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.text.clone())
                .or_else(|| f.questions.iter().find(|q| q.id == id).map(|q| q.prompt.clone()));
            if let Some(s) = state {
                s.update(cx, |s, cx| s.focus(window, cx));
            }
        }
        let draft = self.form_draft(cx);
        let changes = form_changes(&draft, &saved);
        if changes > 0 {
            self.bar = Some(bar_with_error(
                "join-form-bar",
                changes,
                self.pages.join.saving,
                self.pages.join.error.as_deref(),
                p,
                cx,
                move |this, window, cx| {
                    if let Some(saved) = this.pages.join.saved.clone() {
                        this.take_form(saved, window, cx);
                    }
                    this.pages.join.error = None;
                    cx.notify();
                },
                |this, window, cx| this.save_join_form(window, cx),
            ));
        }
        let starters: Vec<String> = STARTERS.iter().map(|k| t(k)).collect();
        let typed: Vec<String> = self.pages.join.rules.iter().map(|r| r.text.read(cx).value().to_string()).collect();
        let ideas: Vec<String> = ideas(&typed, &starters).into_iter().cloned().collect();
        let count = |n: usize, max: usize| {
            div()
                .flex_none()
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(format!("{n}/{max}"))
        };
        let title = |title: String, hint: String| {
            div()
                .min_w_0()
                .child(div().font_weight(FontWeight::EXTRA_BOLD).line_height(px(24.0)).child(title))
                .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(hint))
        };

        // The rules.
        let mut list = div().flex().flex_col().gap(px(8.0));
        let rules_len = self.pages.join.rules.len();
        for n in 0..rules_len {
            let (id, state) = {
                let r = &self.pages.join.rules[n];
                (r.id, r.text.clone())
            };
            let text = state.read(cx).value().to_string();
            let on = focused(&state, window, cx);
            let (hover, fg, danger) = (p.muted, p.foreground, alpha(p.destructive, 0.1));
            let destructive = p.destructive;
            let drag = RuleDrag { id, index: n, text: text.clone() };
            let primary = alpha(p.primary, 0.5);
            let row = div()
                .id(SharedString::from(format!("rule-{id}")))
                .group(SharedString::from(format!("rule-{id}")))
                .flex()
                .items_center()
                .gap(px(8.0))
                .p(px(6.0))
                .pl(px(4.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(if on { alpha(p.primary, 0.5) } else { p.border.into() })
                .bg(alpha(p.background, 0.5))
                .drag_over::<RuleDrag>(move |s, _, _, _| s.border_color(primary))
                .on_drop(cx.listener(move |this, d: &RuleDrag, _, cx| {
                    let rules = &mut this.pages.join.rules;
                    if let Some(from) = rules.iter().position(|r| r.id == d.id) {
                        let rule = rules.remove(from);
                        rules.insert(n.min(rules.len()), rule);
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .id(SharedString::from(format!("rule-grip-{id}")))
                        .w(px(24.0))
                        .h(px(32.0))
                        .flex_none()
                        .rounded(radius_lg())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .cursor_grab()
                        .hover(move |s| s.bg(hover).text_color(fg))
                        .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                        .child(icon("grip-vertical").size(px(16.0))),
                )
                .child(motion::once(
                    number(n, p),
                    SharedString::from(format!("rule-n-{id}-{n}")),
                    Duration::from_millis(240),
                    |el, t| el.opacity(0.4 + 0.6 * t),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(36.0))
                        .flex()
                        .items_center()
                        .px(px(2.0))
                        .child(Input::new(&state).appearance(false)),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("rule-x-{id}")))
                        .size(px(32.0))
                        .flex_none()
                        .rounded(radius_lg())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .opacity(0.6)
                        .group_hover(SharedString::from(format!("rule-{id}")), |s| s.opacity(1.0))
                        .cursor_pointer()
                        .hover(move |s| s.bg(danger).text_color(destructive))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pages.join.rules.retain(|r| r.id != id);
                            cx.notify();
                        }))
                        .child(icon("x").size(px(16.0))),
                );
            list = list.child(motion::rise(row, SharedString::from(format!("rule-in-{id}")), Duration::ZERO, 6.0));
        }
        let ideas_box = (!ideas.is_empty()).then(|| {
            let hover = alpha(p.primary, 0.5);
            let primary = p.primary;
            super::pages::slide_in(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(16.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_dashed()
                    .border_color(p.border)
                    .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(
                        if rules_len > 0 {
                            t("serversettings.joinForm.moreStarters")
                        } else {
                            t("serversettings.joinForm.noRules")
                        },
                    ))
                    .child(div().flex().flex_wrap().gap(px(8.0)).children(ideas.into_iter().enumerate().map(
                        |(k, text)| {
                            let add = text.clone();
                            div()
                                .id(SharedString::from(format!("starter-{k}")))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .rounded_full()
                                .border_1()
                                .border_color(p.border)
                                .px(px(12.0))
                                .py(px(4.0))
                                .text_xs()
                                .line_height(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .cursor_pointer()
                                .hover(move |s| s.border_color(hover).text_color(primary).translate_y(px(-2.0)))
                                .active(|s| s.scale(0.95))
                                .on_click(
                                    cx.listener(move |this, _, window, cx| this.add_join_rule(&add, false, window, cx)),
                                )
                                .child(icon("plus").size(px(12.0)))
                                .child(text)
                        },
                    ))),
                "rule-ideas",
            )
        });
        let full = rules_len >= MAX_RULES;
        let add_rule = button("add-rule", t("serversettings.joinForm.addRule"), Some("plus"), Look::Outline, false, p)
            .rounded(radius_xl())
            .font_weight(FontWeight::BOLD)
            .when(full, |el| el.opacity(0.5))
            .when(!full, |el| el.on_click(cx.listener(|this, _, window, cx| this.add_join_rule("", true, window, cx))));
        let rules = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .pb(px(24.0))
            .border_b_1()
            .border_color(alpha(p.border, 0.7))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(px(12.0))
                    .child(title(t("serversettings.nav.rules"), t("serversettings.joinForm.rulesHint")))
                    .child(count(rules_len, MAX_RULES)),
            )
            .child(list)
            .children(ideas_box)
            .child(div().flex().child(add_rule));

        // The questions.
        let mut cards = div().flex().flex_col().gap(px(12.0));
        let questions_len = self.pages.join.questions.len();
        for n in 0..questions_len {
            let (id, state, paragraph, required) = {
                let q = &self.pages.join.questions[n];
                (q.id, q.prompt.clone(), q.paragraph, q.required)
            };
            let (danger, destructive) = (alpha(p.destructive, 0.1), p.destructive);
            let on = focused(&state, window, cx);
            let card = div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(if on { alpha(p.primary, 0.5) } else { p.border.into() })
                .bg(alpha(p.background, 0.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(16.0))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .text_color(p.muted_foreground)
                                .child(
                                    t_with("serversettings.joinForm.question", &[("index", Arg::Num(n as i64 + 1))])
                                        .to_uppercase(),
                                ),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .id(SharedString::from(format!("question-x-{id}")))
                                .size(px(32.0))
                                .rounded(radius_lg())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted_foreground)
                                .cursor_pointer()
                                .hover(move |s| s.bg(danger).text_color(destructive))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pages.join.questions.retain(|q| q.id != id);
                                    cx.notify();
                                }))
                                .child(icon("trash").size(px(16.0))),
                        ),
                )
                .child(boxed(Input::new(&state).appearance(false), 40.0, on, p))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(16.0))
                        .child(segmented(
                            LENGTH_IDS[n.min(LENGTH_IDS.len() - 1)],
                            vec![
                                (t("serversettings.joinForm.line"), Some("minus")),
                                (t("serversettings.joinForm.paragraphs"), Some("align-left")),
                            ],
                            usize::from(paragraph),
                            112.0,
                            p,
                            window,
                            cx,
                            move |this: &mut Self, i, cx| {
                                if let Some(q) = this.pages.join.questions.iter_mut().find(|q| q.id == id) {
                                    q.paragraph = i == 1;
                                }
                                cx.notify();
                            },
                        ))
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(t("serversettings.joinForm.required"))
                                .child(crate::ui::settings_controls::switch(
                                    SharedString::from(format!("question-required-{id}")),
                                    required,
                                    false,
                                    p,
                                    window,
                                    cx,
                                    move |this: &mut Self, on, cx| {
                                        if let Some(q) = this.pages.join.questions.iter_mut().find(|q| q.id == id) {
                                            q.required = on;
                                        }
                                        cx.notify();
                                    },
                                )),
                        ),
                );
            cards =
                cards.child(motion::rise(card, SharedString::from(format!("question-in-{id}")), Duration::ZERO, 12.0));
        }
        let full = questions_len >= MAX_QUESTIONS;
        let add_question =
            button("add-question", t("serversettings.joinForm.addQuestion"), Some("plus"), Look::Outline, false, p)
                .rounded(radius_xl())
                .font_weight(FontWeight::BOLD)
                .when(full, |el| el.opacity(0.5))
                .when(!full, |el| {
                    el.on_click(cx.listener(|this, _, window, cx| {
                        let q = this.new_question(
                            &pb::JoinQuestion { prompt: String::new(), paragraph: false, required: true },
                            window,
                            cx,
                        );
                        this.pages.join.focus = Some(q.id);
                        this.pages.join.questions.push(q);
                        cx.notify();
                    }))
                });
        let not_asked = (!server.applications).then(|| {
            super::pages::slide_in(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.muted, 0.6))
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.joinForm.notAsked")),
                    )
                    .child(
                        button("turn-on-apply", t("serversettings.joinForm.turnOnApply"), None, Look::Outline, true, p)
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .on_click(cx.listener(|this, _, _, cx| this.choose(Page::Access, None, cx))),
                    ),
                "join-not-asked",
            )
        });
        let questions = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .pt(px(24.0))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(px(12.0))
                    .child(title(t("serversettings.joinForm.questions"), t("serversettings.joinForm.questionsHint")))
                    .child(count(questions_len, MAX_QUESTIONS)),
            )
            .children(not_asked)
            .child(cards)
            .child(div().flex().child(add_question));
        let form =
            div().flex().flex_col().child(self.mark("rules", rules, p)).child(self.mark("questions", questions, p));
        let shown_questions = if server.applications { draft.questions.clone() } else { Vec::new() };
        let preview = self.form_preview(&draft.rules, &shown_questions, p, cx);
        let _ = (PROMPT_MAX, RULE_MAX);
        crate::ui::settings_controls::with_preview(form, preview, self.wide, p)
    }

    /// The rules and questions as people will see them on their way in (the web's `FormPreview`).
    fn form_preview(
        &mut self,
        rules: &[String],
        questions: &[pb::JoinQuestion],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut card = super::pages::preview_card(p).flex().flex_col().gap(px(12.0)).p(px(16.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(icon("scroll-text").size(px(14.0)))
                .child(if questions.is_empty() {
                    t("serversettings.joinForm.previewFirstLook")
                } else {
                    t("serversettings.joinForm.previewApplying")
                }),
        );
        if rules.is_empty() {
            card = card.child(
                div()
                    .p(px(12.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.muted, 0.5))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.joinForm.previewNoRules")),
            );
        } else {
            card =
                card.child(div().flex().flex_col().gap(px(8.0)).children(rules.iter().enumerate().map(|(n, rule)| {
                    motion::rise(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(12.0))
                            .p(px(8.0))
                            .rounded(radius_2xl())
                            .bg(alpha(p.muted, 0.5))
                            .text_xs()
                            .line_height(px(16.0))
                            .child(number(n, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .pt(px(2.0))
                                    .child(crate::ui::instance_home::inline_markdown(rule, p.foreground)),
                            ),
                        SharedString::from(format!("preview-rule-{n}-{rule}")),
                        Duration::from_millis(45 * n.min(10) as u64),
                        0.0,
                    )
                })));
        }
        for (n, q) in questions.iter().enumerate() {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .text_xs()
                            .line_height(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .child(q.prompt.clone())
                            .when(q.required, |el| el.child(div().text_color(p.destructive).child(" *"))),
                    )
                    .child(
                        div()
                            .h(px(if q.paragraph { 48.0 } else { 28.0 }))
                            .rounded(radius_lg())
                            .border_1()
                            .border_color(p.border)
                            .bg(alpha(p.background, 0.6)),
                    )
                    .id(SharedString::from(format!("preview-q-{n}"))),
            );
        }
        if !rules.is_empty() {
            let agreed = self.pages.join.agreed;
            card = card.child(crate::ui::join::agree_check(agreed, &t("join.rules.agree"), p).text_xs().on_click(
                cx.listener(|this, _, _, cx| {
                    this.pages.join.agreed = !this.pages.join.agreed;
                    cx.notify();
                }),
            ));
        }
        card.into_any_element()
    }
}
