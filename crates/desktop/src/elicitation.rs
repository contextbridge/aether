use crate::theme::DesktopTheme;
use crate::threads::Thread;
use crate::ui::UiTheme;
use crate::ui::{Button, ButtonVariant, StyledExt, SurfaceExt, Typography, TypographyExt, text_field, text_input};
use acp_utils::elicitation::{FieldInput, FormError, FormField, FormFieldKind, accept_content};
use client_core::elicitation::{Elicitation, ElicitationAnswer, ElicitationId};
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*,
};
use gpui_base::input::{Enter, InputState};

pub(crate) struct ElicitationCard {
    thread: Entity<Thread>,
    draft: Option<(ElicitationId, Draft)>,
    _subscription: Subscription,
}

pub(crate) struct OpenReview;

enum Draft {
    Form { fields: Vec<DraftField>, error: Option<FormError> },
    Url(String),
    ArtifactReview(Option<String>),
}

struct DraftField {
    field: FormField,
    answer: FieldAnswer,
}

enum FieldAnswer {
    Typed(Entity<InputState>),
    Picked(FieldInput),
}

impl ElicitationCard {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe_in(&thread, window, |this, _, window, cx| this.sync(window, cx));
        let mut card = Self { thread, draft: None, _subscription: subscription };
        card.sync(window, cx);
        card
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let elicitation = self.thread.read(cx).elicitation();
        if elicitation.map(|(id, _)| id) == self.draft.as_ref().map(|(id, _)| *id) {
            return;
        }
        let elicitation = elicitation.map(|(id, elicitation)| (id, elicitation.clone()));
        self.draft = elicitation.map(|(id, elicitation)| {
            let draft = match elicitation {
                Elicitation::Form { fields, .. } => Draft::form(fields, window, cx),
                Elicitation::Url { url, .. } => Draft::Url(url),
                Elicitation::ArtifactReview(meta) => {
                    Draft::ArtifactReview(meta.path.map(|path| path.display().to_string()))
                }
            };
            (id, draft)
        });
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some((_, Draft::Form { fields, error })) = &mut self.draft else {
            return;
        };
        match accept_content(fields.iter().map(|draft| (&draft.field, draft.answer.input(cx)))) {
            Ok(content) => self.answer(ElicitationAnswer::Accept(Some(content)), cx),
            Err(refused) => {
                *error = Some(refused);
                cx.notify();
            }
        }
    }

    fn primary_action(&mut self, cx: &mut Context<Self>) {
        match &self.draft {
            Some((_, Draft::Form { .. })) => self.submit(cx),
            Some((_, Draft::Url(url))) => {
                cx.open_url(url);
                self.answer(ElicitationAnswer::Accept(None), cx);
            }
            Some((_, Draft::ArtifactReview(_))) => cx.emit(OpenReview),
            None => {}
        }
    }

    fn answer(&self, answer: ElicitationAnswer, cx: &mut App) {
        if let Some((id, _)) = &self.draft {
            let id = *id;
            Thread::with_core(&self.thread, cx, |core| core.answer_elicitation(id, answer));
        }
    }

    fn pick(&mut self, index: usize, option: usize, cx: &mut Context<Self>) {
        if let Some((_, Draft::Form { fields, .. })) = &mut self.draft
            && let Some(FieldAnswer::Picked(input)) = fields.get_mut(index).map(|draft| &mut draft.answer)
        {
            input.pick(option);
            cx.notify();
        }
    }

    fn render_field(
        index: usize,
        draft: &DraftField,
        refused: Option<&FormError>,
        theme: &UiTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let field = &draft.field;
        let name = field.name.clone();
        let label = if field.required { format!("{} *", field.label) } else { field.label.clone() };
        let control = match (&draft.answer, &field.kind) {
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
                    let selector = format!("elicitation-option-{name}-{option}");
                    let selected = input.is_selected(option);
                    Self::pick_button(selector, choice.title.clone().into(), selected, (index, option), theme, cx)
                }))
                .into_any_element(),
            (FieldAnswer::Picked(_), _) => div().into_any_element(),
        };
        let error = refused.filter(|refused| refused.index == index).map(|refused| {
            let selector = format!("elicitation-error-{name}");
            div()
                .debug_selector(move || selector)
                .typography(Typography::Small)
                .text_color(theme.colors.destructive)
                .child(refused.to_string())
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().typography(Typography::Label).child(label))
            .children(field.description.clone().map(|description| div().muted(theme).child(description)))
            .child(control)
            .children(error)
            .into_any_element()
    }

    fn pick_button(
        selector: String,
        label: SharedString,
        selected: bool,
        (index, option): (usize, usize),
        theme: &UiTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new(SharedString::from(selector), label, *theme)
            .selected(selected)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pick(index, option, cx)))
            .into_any_element()
    }

    fn render_input(name: &str, input: &Entity<InputState>, theme: &UiTheme, cx: &mut Context<Self>) -> AnyElement {
        let selector = format!("elicitation-field-{name}");
        text_field(input, theme)
            .debug_selector(move || selector)
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.submit(cx)))
            .into_any_element()
    }
}

impl EventEmitter<OpenReview> for ElicitationCard {}

impl Render for ElicitationCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (Some((_, draft)), Some((_, elicitation))) = (&self.draft, self.thread.read(cx).elicitation()) else {
            return div().into_any_element();
        };
        let theme = DesktopTheme::global(cx).ui();
        let message = elicitation.question();
        let (primary_id, primary_label) = draft.primary();
        let body = match draft {
            Draft::Form { fields, error } => div().flex().flex_col().gap_3().children(
                fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| Self::render_field(index, field, error.as_ref(), &theme, cx)),
            ),
            Draft::Url(url) => div().muted(&theme).child(url.clone()),
            Draft::ArtifactReview(path) => div().muted(&theme).children(path.clone()),
        };
        let primary = Button::new(primary_id, primary_label, theme)
            .variant(ButtonVariant::Primary)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.primary_action(cx)));
        let decline = Button::new("elicitation-decline", "Decline", theme)
            .variant(ButtonVariant::Secondary)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.answer(ElicitationAnswer::Decline, cx)));
        let actions = div().flex().justify_end().gap_2().child(primary).child(decline);
        div()
            .debug_selector(|| "elicitation".to_owned())
            .mt_2()
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .elevated(theme)
            .border_color(theme.colors.accent)
            .shadow_md()
            .child(div().typography(Typography::Title).child(message))
            .child(body)
            .child(actions)
            .into_any_element()
    }
}

impl Draft {
    fn primary(&self) -> (&'static str, &'static str) {
        match self {
            Self::Form { .. } => ("elicitation-submit", "Submit"),
            Self::Url(_) => ("elicitation-open-url", "Open link"),
            Self::ArtifactReview(_) => ("elicitation-open-review", "Open review"),
        }
    }

    fn form(fields: Vec<FormField>, window: &mut Window, cx: &mut App) -> Self {
        let fields = fields
            .into_iter()
            .map(|field| {
                let answer = match &field.default_input {
                    FieldInput::Text(text) => FieldAnswer::Typed(text_input(text.clone(), window, cx)),
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
