use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadEvent};
use crate::ui::tokens::rem;
use crate::ui::{PickerItem, SearchPicker, SearchPickerEvent};
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use client_core::composer::prompt_search::PromptSearchState;
use gpui::{Context, Entity, EventEmitter, Subscription, Window, prelude::*};

/// A search through every prompt the agent has been sent, in any session, to send one again.
///
/// The agent answers each query as the user types, and an answer to a query edited away from is
/// dropped.
pub(crate) struct PromptSearch {
    thread: Entity<Thread>,
    picker: Entity<SearchPicker>,
    state: PromptSearchState,
    results: Vec<PromptSearchResult>,
    _subscriptions: [Subscription; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptSearchOutcome {
    Chose(String),
    Dismissed,
}

impl PromptSearch {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = DesktopTheme::global(cx).ui();
        let picker = cx.new(|cx| {
            SearchPicker::new("prompt-search", Vec::new(), theme, cx)
                .unfiltered()
                .searchable("Search past prompts…", window, cx)
                .empty("No matching prompts")
                .max_list_height(rem(RESULTS_HEIGHT))
        });
        let subscriptions = [
            cx.subscribe(&picker, |this, _, event: &SearchPickerEvent, cx| match event {
                SearchPickerEvent::Query(query) => this.search(query, cx),
                SearchPickerEvent::Chose(index) => {
                    cx.emit(PromptSearchOutcome::Chose(this.results[*index].prompt.clone()));
                }
                SearchPickerEvent::Dismissed => cx.emit(PromptSearchOutcome::Dismissed),
            }),
            cx.subscribe(&thread, |this, _, event: &ThreadEvent, cx| {
                if let ThreadEvent::PromptSearchResults { query, result } = event {
                    this.on_answer(query, result, cx);
                }
            }),
        ];
        let search = Self {
            thread,
            picker,
            state: PromptSearchState::default(),
            results: Vec::new(),
            _subscriptions: subscriptions,
        };
        search.show(cx);
        search
    }

    fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = self.state.set_query(query);
        if self.state.is_blank() {
            self.results.clear();
        } else {
            self.thread.update(cx, |thread, cx| thread.search_prompts(query, cx));
        }
        self.show(cx);
    }

    fn on_answer(&mut self, query: &str, result: &Result<PromptSearchResponse, String>, cx: &mut Context<Self>) {
        match result {
            Ok(response) => {
                if let Some(results) = self.state.on_results(response.clone()) {
                    self.results = results;
                }
            }
            Err(error) => {
                self.state.on_failed(query, error.clone());
            }
        }
        self.show(cx);
    }

    fn show(&self, cx: &mut Context<Self>) {
        let items = self
            .results
            .iter()
            .enumerate()
            .map(|(n, result)| {
                PickerItem::new(format!("result-{n}"), result.prompt.lines().next().unwrap_or_default().to_owned())
            })
            .collect();
        let status = if let Some(error) = self.state.error() {
            Some(format!("Search failed: {error}").into())
        } else if self.state.is_loading() {
            Some("Searching…".into())
        } else if self.state.is_blank() {
            Some("Type to search the prompts you have sent".into())
        } else {
            None
        };
        self.picker.update(cx, |picker, cx| {
            picker.set_items(items, cx);
            picker.set_status(status, cx);
        });
    }
}

impl EventEmitter<PromptSearchOutcome> for PromptSearch {}

impl Render for PromptSearch {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.picker.clone()
    }
}

const RESULTS_HEIGHT: f32 = 200.0;
