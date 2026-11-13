use crate::platform::Platform;
use crate::threads::Thread;
use crate::ui::{ButtonVariant, Material, MaterialExt, Theme, UiStyles, button, text_field};
use client_core::elicitation::{
    Elicitation, ElicitationAnswer, ElicitationId, FieldInput, FormError, FormField, FormFieldKind, accept_content,
};
use rouge::core::{AnyView, Styled, View, div, dynamic, px, text, view};
use rouge::reactive::{Callback, Scope, Signal, Stored, effect, memo, signal, stored, untrack};
use std::rc::Rc;

#[derive(Clone, Copy)]
pub(crate) struct ElicitationCard {
    thread: Thread,
    shown: Stored<Option<ElicitationId>>,
    form: Signal<Option<Form>>,
    on_open_review: Callback,
    platform: Stored<Rc<dyn Platform>>,
    scope: Scope,
}

#[derive(Clone)]
struct Form {
    fields: Vec<DraftField>,
    error: Option<FormError>,
}

#[derive(Clone)]
struct DraftField {
    field: FormField,
    answer: FieldAnswer,
}

#[derive(Clone)]
enum FieldAnswer {
    Typed(Signal<String>),
    Picked(FieldInput),
}

impl ElicitationCard {
    pub(crate) fn new(thread: Thread, platform: Rc<dyn Platform>, on_open_review: impl Fn() + 'static) -> Self {
        let card = Self {
            thread,
            shown: stored(None),
            form: signal(None),
            on_open_review: Callback::from(on_open_review),
            platform: stored(platform),
            scope: Scope::new(),
        };
        let id = memo(move || thread.elicitation().map(|(id, _)| id));
        effect(move || {
            let id = id.get();
            untrack(|| card.sync(id));
        });
        card
    }

    pub(crate) fn view(self) -> impl View {
        let thread = self.thread;
        let id = memo(move || thread.elicitation().map(|(id, _)| id));
        dynamic(move || {
            id.get()?;
            let (_, elicitation) = untrack(|| thread.elicitation())?;
            Some(self.card(&elicitation))
        })
    }

    fn card(self, elicitation: &Elicitation) -> AnyView {
        let theme = Theme::current();
        let (primary_id, primary_label) = primary(elicitation);
        let body = match elicitation {
            Elicitation::Url { url, .. } => view! { <div><text small muted>{url.clone()}</text></div> }.into_any(),
            Elicitation::ArtifactReview(meta) => {
                let path = meta.path.as_ref().map(|path| path.display().to_string()).unwrap_or_default();
                view! { <div><text small muted>{path}</text></div> }.into_any()
            }
            Elicitation::Form { .. } => {
                let form = self.form;
                dynamic(move || {
                    let fields = form.with(|form| form.as_ref().map(|form| (form.fields.clone(), form.error.clone())));
                    let (fields, error) = fields.unwrap_or_default();
                    let fields =
                        fields.iter().enumerate().map(|(index, field)| self.field(index, field, error.as_ref()));
                    view! { <div flex col gap=12.0>{..fields.collect::<Vec<_>>()}</div> }
                })
                .into_any()
            }
        };
        view! {
            <div test_id="elicitation" mt=8.0 p=16.0 flex col gap=16.0 material=Material::Glass border=px(1.0) border_color=theme.accent>
                <text title primary>{elicitation.question()}</text>
                {body}
                <div flex justify_end gap=8.0>
                    <button id=primary_id label=primary_label variant=ButtonVariant::Primary
                            on_click={move || self.primary_action()}/>
                    <button id="elicitation-decline" label="Decline" variant=ButtonVariant::Secondary
                            on_click={move || self.answer(ElicitationAnswer::Decline)}/>
                </div>
            </div>
        }
        .into_any()
    }

    fn field(self, index: usize, draft: &DraftField, refused: Option<&FormError>) -> AnyView {
        let field = &draft.field;
        let name = field.name.clone();
        let label = if field.required { format!("{} *", field.label) } else { field.label.clone() };
        let control = match (&draft.answer, &field.kind) {
            (FieldAnswer::Typed(value), _) => {
                text_field(format!("elicitation-field-{name}"), *value, move |_| self.submit()).into_any()
            }
            (FieldAnswer::Picked(FieldInput::Boolean(checked)), _) => {
                let label = if *checked { "Yes" } else { "No" };
                self.pick_button(format!("elicitation-field-{name}"), label.to_owned(), *checked, (index, 0))
            }
            (FieldAnswer::Picked(input), FormFieldKind::Single(options) | FormFieldKind::Multi(options)) => {
                let buttons = options.iter().enumerate().map(|(option, choice)| {
                    let id = format!("elicitation-option-{name}-{option}");
                    self.pick_button(id, choice.title.clone(), input.is_selected(option), (index, option))
                });
                view! { <div flex flex_wrap gap=4.0>{..buttons.collect::<Vec<_>>()}</div> }.into_any()
            }
            (FieldAnswer::Picked(_), _) => ().into_any(),
        };
        let theme = Theme::current();
        let error = refused.filter(|refused| refused.index == index).map(|refused| {
            view! {
                <div test_id={format!("elicitation-error-{name}")}>
                    <text small color=theme.destructive>{refused.to_string()}</text>
                </div>
            }
        });
        let description = field.description.clone().map(|description| view! { <text small muted>{description}</text> });
        view! {
            <div flex col gap=4.0>
                <text label primary>{label}</text>
                {description}
                {control}
                {error}
            </div>
        }
        .into_any()
    }

    fn pick_button(self, id: String, label: String, selected: bool, (index, option): (usize, usize)) -> AnyView {
        view! { <button id label selected=selected on_click={move || self.pick(index, option)}/> }
    }

    fn sync(self, id: Option<ElicitationId>) {
        if self.shown.get() == id {
            return;
        }
        self.shown.set(id);
        let fields = match self.thread.elicitation() {
            Some((_, Elicitation::Form { fields, .. })) => Some(fields),
            _ => None,
        };
        let form = fields.map(|fields| self.scope.run(|| Form::new(fields)));
        self.form.set(form);
    }

    fn submit(self) {
        let inputs = self.form.with_untracked(|form| {
            form.as_ref().map(|form| {
                form.fields.iter().map(|draft| (draft.field.clone(), draft.answer.input())).collect::<Vec<_>>()
            })
        });
        let Some(inputs) = inputs else {
            return;
        };
        match accept_content(inputs.iter().map(|(field, input)| (field, input.clone()))) {
            Ok(answer) => self.answer(answer),
            Err(refused) => self.form.update(|form| {
                if let Some(form) = form {
                    form.error = Some(refused);
                }
            }),
        }
    }

    fn primary_action(self) {
        match self.thread.elicitation() {
            Some((_, Elicitation::Form { .. })) => self.submit(),
            Some((_, Elicitation::Url { url, .. })) => {
                self.platform.with(|platform| platform.open(&url));
                self.answer(ElicitationAnswer::Accept(None));
            }
            Some((_, Elicitation::ArtifactReview(_))) => self.on_open_review.call(),
            None => {}
        }
    }

    fn answer(self, answer: ElicitationAnswer) {
        if let Some(id) = self.shown.get() {
            self.thread.update_core(|core| core.answer_elicitation(id, answer));
        }
    }

    fn pick(self, index: usize, option: usize) {
        self.form.update(|form| {
            if let Some(FieldAnswer::Picked(input)) =
                form.as_mut().and_then(|form| form.fields.get_mut(index)).map(|draft| &mut draft.answer)
            {
                input.pick(option);
            }
        });
    }
}

impl Form {
    fn new(fields: Vec<FormField>) -> Self {
        let fields = fields
            .into_iter()
            .map(|field| {
                let answer = match &field.default_input {
                    FieldInput::Text(text) => FieldAnswer::Typed(signal(text.clone())),
                    input => FieldAnswer::Picked(input.clone()),
                };
                DraftField { field, answer }
            })
            .collect();
        Self { fields, error: None }
    }
}

impl FieldAnswer {
    fn input(&self) -> FieldInput {
        match self {
            Self::Typed(value) => FieldInput::Text(value.get_untracked()),
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
