use crate::theme::DesktopTheme;
use crate::threads::Thread;
use crate::ui::UiTheme;
use crate::ui::{Button, ButtonVariant, StyledExt, SurfaceExt, Typography, TypographyExt, text_field, text_input};
use client_core::elicitation::{
    Elicitation, ElicitationAnswer, ElicitationId, FieldInput, FormError, FormField, FormFieldKind, accept_content,
};
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*,
};
use gpui_base::input::{Enter, InputState};

pub(crate) struct ElicitationCard {
    thread: Entity<Thread>,
    shown: Option<ElicitationId>,
    form: Option<Form>,
    _subscription: Subscription,
}

pub(crate) struct OpenReview;

struct Form {
    fields: Vec<DraftField>,
    error: Option<FormError>,
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
        let mut card = Self { thread, shown: None, form: None, _subscription: subscription };
        card.sync(window, cx);
        card
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let elicitation = self.thread.read(cx).elicitation();
        let shown = elicitation.map(|(id, _)| id);
        if shown == self.shown {
            return;
        }
        let fields = match elicitation {
            Some((_, Elicitation::Form { fields, .. })) => Some(fields.clone()),
            _ => None,
        };
        self.shown = shown;
        self.form = fields.map(|fields| Form::new(fields, window, cx));
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        match accept_content(form.fields.iter().map(|draft| (&draft.field, draft.answer.input(cx)))) {
            Ok(answer) => self.answer(answer, cx),
            Err(refused) => {
                form.error = Some(refused);
                cx.notify();
            }
        }
    }

    fn primary_action(&mut self, cx: &mut Context<Self>) {
        match self.thread.read(cx).elicitation() {
            Some((_, Elicitation::Form { .. })) => self.submit(cx),
            Some((_, Elicitation::Url { url, .. })) => {
                cx.open_url(url);
                self.answer(ElicitationAnswer::Accept(None), cx);
            }
            Some((_, Elicitation::ArtifactReview(_))) => cx.emit(OpenReview),
            None => {}
        }
    }

    fn answer(&self, answer: ElicitationAnswer, cx: &mut App) {
        if let Some(id) = self.shown {
            Thread::with_core(&self.thread, cx, |core| core.answer_elicitation(id, answer));
        }
    }

    fn pick(&mut self, index: usize, option: usize, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form
            && let Some(FieldAnswer::Picked(input)) = form.fields.get_mut(index).map(|draft| &mut draft.answer)
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
        let Some((_, elicitation)) = self.thread.read(cx).elicitation() else {
            return div().into_any_element();
        };
        let theme = DesktopTheme::global(cx).ui();
        let message = elicitation.question();
        let (primary_id, primary_label) = primary(elicitation);
        let body = match elicitation {
            Elicitation::Url { url, .. } => div().muted(&theme).child(url.clone()),
            Elicitation::ArtifactReview(meta) => {
                div().muted(&theme).children(meta.path.as_ref().map(|path| path.display().to_string()))
            }
            Elicitation::Form { .. } => {
                let (fields, error) =
                    self.form.as_ref().map_or((&[][..], None), |form| (&form.fields, form.error.as_ref()));
                let fields = fields.iter().enumerate();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(fields.map(|(index, field)| Self::render_field(index, field, error, &theme, cx)))
            }
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

impl Form {
    fn new(fields: Vec<FormField>, window: &mut Window, cx: &mut App) -> Self {
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
        Self { fields, error: None }
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

fn primary(elicitation: &Elicitation) -> (&'static str, &'static str) {
    match elicitation {
        Elicitation::Form { .. } => ("elicitation-submit", "Submit"),
        Elicitation::Url { .. } => ("elicitation-open-url", "Open link"),
        Elicitation::ArtifactReview(_) => ("elicitation-open-review", "Open review"),
    }
}
