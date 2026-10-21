//! The card a thread shows while its agent waits on a question: a form to fill in, or a link to
//! answer at.

use crate::theme::DesktopTheme;
use crate::threads::Thread;
use acp_utils::elicitation::{FieldInput, FormError, FormField, FormFieldKind, accept_content, form_fields};
use agent_client_protocol::schema::v2::ElicitationMode;
use clankerdiff_gpui::ui::prelude::{Button, ButtonVariant, MutedText, UiTheme};
use client_core::elicitation::ElicitationResponder;
use client_core::session_core::Elicitation;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, MouseButton, SharedString, Subscription, Window, div,
    prelude::*,
};
use gpui_base::input::{Enter, Input, InputState};

/// Shows the question a thread's agent waits on, above the composer, and sends the user's answer.
///
/// A form is answered with Submit, a link by opening it, and either can be declined. A question
/// the card cannot answer, such as an artifact review, can only be declined.
pub(crate) struct ElicitationCard {
    thread: Entity<Thread>,
    /// The answer being written, while the thread has a question waiting.
    draft: Option<Draft>,
    _subscription: Subscription,
}

/// What the agent asks the user, in a line.
pub(crate) fn question(elicitation: &Elicitation) -> String {
    match elicitation {
        Elicitation::Request(request) => request.message.clone(),
        Elicitation::ArtifactReview(review) => format!("Review {}", review.title),
    }
}

enum Draft {
    /// Each field of the form with its answer so far, and why the last submission was refused.
    Form { fields: Vec<DraftField>, error: Option<FormError> },
    /// A page the user answers at.
    Url(String),
    /// A question the card can only decline.
    Unsupported,
}

struct DraftField {
    field: FormField,
    answer: FieldAnswer,
}

enum FieldAnswer {
    /// Text, integer, and number fields are typed.
    Typed(Entity<InputState>),
    /// Boolean and select fields are clicked.
    Picked(FieldInput),
}

impl ElicitationCard {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe_in(&thread, window, |this, _, window, cx| this.sync(window, cx));
        let mut card = Self { thread, draft: None, _subscription: subscription };
        card.sync(window, cx);
        card
    }

    /// Starts a draft when a question arrives, and drops it once the question is answered or
    /// withdrawn.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = match self.thread.read(cx).elicitation() {
            None => None,
            Some(_) if self.draft.is_some() => return,
            Some(Elicitation::Request(request)) => Some(match &request.mode {
                ElicitationMode::Form(form) => form_fields(&form.requested_schema)
                    .map_or(Draft::Unsupported, |fields| Draft::form(fields, window, cx)),
                ElicitationMode::Url(url) => Draft::Url(url.url.clone()),
                _ => Draft::Unsupported,
            }),
            Some(Elicitation::ArtifactReview(_)) => Some(Draft::Unsupported),
        };
        self.draft = draft;
        cx.notify();
    }

    /// Sends the form's answers, unless one cannot be sent, which is shown beside its field.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(Draft::Form { fields, error }) = &mut self.draft else {
            return;
        };
        match accept_content(fields.iter().map(|draft| (&draft.field, draft.answer.input(cx)))) {
            Ok(content) => {
                self.thread.update(cx, |thread, cx| thread.answer(|answer| answer.accept(Some(content)), cx));
            }
            Err(refused) => {
                *error = Some(refused);
                cx.notify();
            }
        }
    }

    /// Opens the page the agent asks the user to answer at, and tells the agent it was opened.
    fn open_url(&mut self, cx: &mut Context<Self>) {
        if let Some(Draft::Url(url)) = &self.draft {
            cx.open_url(url);
            self.thread.update(cx, |thread, cx| thread.answer(|answer| answer.accept(None), cx));
        }
    }

    fn decline(&mut self, cx: &mut Context<Self>) {
        self.thread.update(cx, |thread, cx| thread.answer(ElicitationResponder::decline, cx));
    }

    /// Clicks option `option` of field `index`: a boolean field flips, a single-select field
    /// chooses the option, and a multi-select field checks or unchecks it.
    fn pick(&mut self, index: usize, option: usize, cx: &mut Context<Self>) {
        let Some(Draft::Form { fields, .. }) = &mut self.draft else {
            return;
        };
        let Some(FieldAnswer::Picked(input)) = fields.get_mut(index).map(|draft| &mut draft.answer) else {
            return;
        };
        match input {
            FieldInput::Boolean(checked) => *checked = !*checked,
            FieldInput::Single(chosen) => *chosen = option,
            FieldInput::Multi(checked) => {
                if let Some(checked) = checked.get_mut(option) {
                    *checked = !*checked;
                }
            }
            FieldInput::Text(_) => {}
        }
        cx.notify();
    }

    fn render_field(
        index: usize,
        draft: &DraftField,
        refused: Option<&FormError>,
        theme: &UiTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let field = &draft.field;
        let name = field.name().to_owned();
        let label = if field.required() { format!("{} *", field.label()) } else { field.label().to_owned() };
        let control = match (&draft.answer, field.kind()) {
            (FieldAnswer::Typed(input), _) => Self::render_input(&name, input, theme, cx),
            (FieldAnswer::Picked(FieldInput::Boolean(checked)), _) => {
                let label = if *checked { "Yes" } else { "No" };
                Self::pick_button(format!("elicitation-field-{name}"), label.into(), *checked, (index, 0), theme, cx)
            }
            (FieldAnswer::Picked(input), FormFieldKind::Single(options) | FormFieldKind::Multi(options)) => div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(options.iter().enumerate().map(|(option, choice)| {
                    let selected = match input {
                        FieldInput::Single(chosen) => *chosen == option,
                        FieldInput::Multi(checked) => checked.get(option).copied().unwrap_or_default(),
                        FieldInput::Text(_) | FieldInput::Boolean(_) => false,
                    };
                    let selector = format!("elicitation-option-{name}-{option}");
                    Self::pick_button(selector, choice.title.clone().into(), selected, (index, option), theme, cx)
                }))
                .into_any_element(),
            (FieldAnswer::Picked(_), _) => div().into_any_element(),
        };
        let error = refused.filter(|refused| refused.index == index).map(|refused| {
            let selector = format!("elicitation-error-{name}");
            div()
                .debug_selector(move || selector)
                .text_sm()
                .text_color(theme.colors.destructive)
                .child(refused.to_string())
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
            .children(
                field
                    .description()
                    .map(|description| div().text_sm().child(MutedText::new(description.to_owned(), *theme))),
            )
            .child(control)
            .children(error)
            .into_any_element()
    }

    /// A button highlighted while `selected`, which clicks option `option` of field `index`.
    fn pick_button(
        selector: String,
        label: SharedString,
        selected: bool,
        (index, option): (usize, usize),
        theme: &UiTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = Button::new(SharedString::from(selector.clone()), label, *theme)
            .selected(selected)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pick(index, option, cx)));
        tagged(selector, button).into_any_element()
    }

    /// A one-line text field, where Enter submits the form.
    fn render_input(name: &str, input: &Entity<InputState>, theme: &UiTheme, cx: &mut Context<Self>) -> AnyElement {
        let selector = format!("elicitation-field-{name}");
        let focus = input.clone();
        div()
            .debug_selector(move || selector)
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.canvas)
            .cursor_text()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.update(cx, |input, cx| input.focus(window, cx));
                cx.stop_propagation();
            })
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.submit(cx)))
            .child(Input::new(input))
            .into_any_element()
    }
}

impl Render for ElicitationCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (Some(draft), Some(elicitation)) = (&self.draft, self.thread.read(cx).elicitation()) else {
            return div().into_any_element();
        };
        let theme = DesktopTheme::global(cx).ui();
        let message = question(elicitation);
        let body = match draft {
            Draft::Form { fields, error } => {
                let rendered: Vec<AnyElement> = fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| Self::render_field(index, field, error.as_ref(), &theme, cx))
                    .collect();
                div().flex().flex_col().gap_3().children(rendered).into_any_element()
            }
            Draft::Url(url) => div().text_sm().child(MutedText::new(url.clone(), theme)).into_any_element(),
            Draft::Unsupported => div()
                .text_sm()
                .child(MutedText::new("This question can't be answered here yet.", theme))
                .into_any_element(),
        };
        let primary = match draft {
            Draft::Form { .. } => Some(tagged(
                "elicitation-submit",
                Button::new("elicitation-submit", "Submit", theme)
                    .variant(ButtonVariant::Primary)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx))),
            )),
            Draft::Url(_) => Some(tagged(
                "elicitation-open-url",
                Button::new("elicitation-open-url", "Open link", theme)
                    .variant(ButtonVariant::Primary)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_url(cx))),
            )),
            Draft::Unsupported => None,
        };
        let decline = tagged(
            "elicitation-decline",
            Button::new("elicitation-decline", "Decline", theme)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.decline(cx))),
        );
        let actions = div().flex().justify_end().gap_2().children(primary).child(decline);
        div()
            .debug_selector(|| "elicitation".to_owned())
            .flex_none()
            .mx_3()
            .mb_2()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_md()
            .border_1()
            .border_color(theme.colors.accent)
            .bg(theme.colors.surface)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(message))
            .child(body)
            .child(actions)
            .into_any_element()
    }
}

impl Draft {
    fn form(fields: Vec<FormField>, window: &mut Window, cx: &mut App) -> Self {
        let style = DesktopTheme::global(cx).editor_style();
        let fields = fields
            .into_iter()
            .map(|field| {
                let answer = match field.default_input() {
                    FieldInput::Text(text) => {
                        let text = text.clone();
                        FieldAnswer::Typed(cx.new(|cx| {
                            let mut input = InputState::new(window, cx).default_value(text);
                            input.set_editor_style(style.clone());
                            input
                        }))
                    }
                    input => FieldAnswer::Picked(input.clone()),
                };
                DraftField { field, answer }
            })
            .collect();
        Self::Form { fields, error: None }
    }
}

impl FieldAnswer {
    fn input(&self, cx: &App) -> FieldInput {
        match self {
            Self::Typed(input) => FieldInput::Text(input.read(cx).value().to_string()),
            Self::Picked(input) => input.clone(),
        }
    }
}

/// Wraps `button` in an element tagged `selector`, since buttons cannot be tagged themselves.
fn tagged(selector: impl Into<String>, button: Button) -> impl IntoElement {
    let selector = selector.into();
    div().debug_selector(move || selector).child(button)
}
