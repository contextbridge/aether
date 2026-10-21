use crate::theme::DesktopTheme;
use crate::threads::{PromptSearchEvent, Thread};
use acp_utils::notifications::PromptSearchResult;
use clankerdiff_gpui::ui::prelude::{ListRow, MutedText, SelectionState};
use client_core::composer::prompt_search::PromptSearchState;
use gpui::{ClickEvent, Context, ElementId, Entity, EventEmitter, Subscription, Window, div, prelude::*, px};
use gpui_base::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};

/// A search through every prompt the agent has been sent, in any session, to send one again.
///
/// The agent answers each query as the user types, and an answer to a query edited away from is
/// dropped. ↑ and ↓ move the highlight, Enter or a click chooses a prompt, and Escape dismisses
/// the search. It is tagged `prompt-search`, and each result `prompt-search-result-<n>`.
pub(crate) struct PromptSearch {
    thread: Entity<Thread>,
    input: Entity<InputState>,
    state: PromptSearchState,
    results: Vec<PromptSearchResult>,
    highlighted: usize,
    _subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptSearchOutcome {
    Chose(String),
    Dismissed,
}

impl PromptSearch {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = DesktopTheme::global(cx).editor_style();
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Search past prompts…");
            input.set_editor_style(style);
            input
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions = vec![
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search(cx);
                }
            }),
            cx.subscribe(&thread, |this, _, event: &PromptSearchEvent, cx| this.on_answer(event, cx)),
        ];
        Self {
            thread,
            input,
            state: PromptSearchState::default(),
            results: Vec::new(),
            highlighted: 0,
            _subscriptions: subscriptions,
        }
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.state.set_query(&self.input.read(cx).value());
        if self.state.is_blank() {
            self.results.clear();
        } else {
            self.thread.update(cx, |thread, cx| thread.search_prompts(query, cx));
        }
        cx.notify();
    }

    fn on_answer(&mut self, event: &PromptSearchEvent, cx: &mut Context<Self>) {
        match &event.result {
            Ok(response) => {
                if let Some(results) = self.state.on_results(response.clone()) {
                    self.results = results;
                    self.highlighted = 0;
                }
            }
            Err(error) => {
                self.state.on_failed(&event.query, error.clone());
            }
        }
        cx.notify();
    }

    /// Moves the highlight `by` results, stopping at either end of the list.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.results.len().saturating_sub(1);
        self.highlighted = self.highlighted.saturating_add_signed(by).min(last);
        cx.notify();
    }

    fn choose(&mut self, position: usize, cx: &mut Context<Self>) {
        if let Some(result) = self.results.get(position) {
            cx.emit(PromptSearchOutcome::Chose(result.prompt.clone()));
        }
    }

    fn status(&self) -> Option<String> {
        if let Some(error) = self.state.error() {
            Some(format!("Search failed: {error}"))
        } else if self.state.is_loading() {
            Some("Searching…".to_owned())
        } else if self.state.is_blank() {
            Some("Type to search the prompts you have sent".to_owned())
        } else if self.results.is_empty() {
            Some("No matching prompts".to_owned())
        } else {
            None
        }
    }
}

impl EventEmitter<PromptSearchOutcome> for PromptSearch {}

impl Render for PromptSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let mut results = div().id("prompt-search-results").max_h(px(RESULTS_HEIGHT)).overflow_y_scroll();
        for (position, result) in self.results.iter().enumerate() {
            let state = if position == self.highlighted { SelectionState::Selected } else { SelectionState::None };
            let prompt = result.prompt.lines().next().unwrap_or_default().to_owned();
            let row = ListRow::new(ElementId::NamedInteger("prompt-search-row".into(), position as u64), theme)
                .state(state)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(position, cx)))
                .child(div().flex_1().min_w_0().truncate().child(prompt));
            results =
                results.child(div().debug_selector(move || format!("prompt-search-result-{position}")).child(row));
        }
        div()
            .debug_selector(|| "prompt-search".to_owned())
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(theme.colors.accent)
            .bg(theme.colors.surface)
            // Captured on the way down to the search field, which would otherwise move its cursor.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.choose(this.highlighted, cx)))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(PromptSearchOutcome::Dismissed)))
            .child(div().px_1().child(Input::new(&self.input)))
            .children(self.status().map(|status| div().px_1().text_sm().child(MutedText::new(status, theme))))
            .child(results)
    }
}

const RESULTS_HEIGHT: f32 = 200.0;
