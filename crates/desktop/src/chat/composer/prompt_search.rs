use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadEvent};
use crate::ui::tokens::{COMPOSER_POPUP_HEIGHT, rem};
use crate::ui::{PickerItem, SearchPicker, SearchPickerEvent};
use client_core::composer::prompt_search::PromptSearchStatus;
use client_core::session_core::SessionCore;
use gpui::{Context, Entity, EventEmitter, Subscription, Window, prelude::*};

pub(crate) struct PromptSearch {
    thread: Entity<Thread>,
    picker: Entity<SearchPicker>,
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
                .max_list_height(rem(COMPOSER_POPUP_HEIGHT))
        });
        let subscriptions = [
            cx.subscribe(&picker, |this, _, event: &SearchPickerEvent, cx| match event {
                SearchPickerEvent::Query(query) => this.search(query, cx),
                SearchPickerEvent::Chose(index) => {
                    let search = this.thread.read(cx).core().map(SessionCore::prompt_search);
                    if let Some(result) = search.and_then(|search| search.results().get(*index)) {
                        cx.emit(PromptSearchOutcome::Chose(result.prompt.clone()));
                    }
                }
                SearchPickerEvent::Dismissed => cx.emit(PromptSearchOutcome::Dismissed),
                SearchPickerEvent::Highlighted(_) => {}
            }),
            cx.subscribe(&thread, |this, _, event: &ThreadEvent, cx| {
                if matches!(event, ThreadEvent::PromptSearchUpdated) {
                    this.show(cx);
                }
            }),
        ];
        let search = Self { thread, picker, _subscriptions: subscriptions };
        search.search("", cx);
        search
    }

    fn search(&self, query: &str, cx: &mut Context<Self>) {
        Thread::with_core(&self.thread, cx, |core| core.search_prompts(query));
        self.show(cx);
    }

    fn show(&self, cx: &mut Context<Self>) {
        let Some(search) = self.thread.read(cx).core().map(SessionCore::prompt_search) else {
            return;
        };
        let items = search
            .results()
            .iter()
            .enumerate()
            .map(|(n, result)| {
                PickerItem::new(format!("result-{n}"), result.prompt.lines().next().unwrap_or_default().to_owned())
            })
            .collect();
        let status = match search.status() {
            PromptSearchStatus::Failed(error) => Some(format!("Search failed: {error}").into()),
            PromptSearchStatus::Searching => Some("Searching…".into()),
            PromptSearchStatus::Blank => Some("Type to search the prompts you have sent".into()),
            PromptSearchStatus::NoMatches | PromptSearchStatus::Results => None,
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
