use super::markdown::{MarkdownRenderer, StreamedMarkdown};
use super::notice::notice;
use super::thinking::thinking_block;
use super::tool_card::tool_card;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadStatus};
use acp_utils::conversation::{
    Conversation, ConversationContent, ConversationId, ConversationItem, ConversationItemId, Revision,
};
use clankerdiff_gpui::ui::prelude::{Button, ButtonVariant, ControlSize, EmptyState, NoticeTone, Surface, UiTheme};
use clankerdiff_syntax::SyntaxHighlighter;
use client_core::session_core::SessionCore;
use gpui::{AnyElement, Context, Entity, FollowMode, ListAlignment, ListState, Window, div, list, prelude::*, px};
use std::collections::{HashMap, HashSet};

/// A thread's conversation, one list row per item: prompts, the agent's replies as Markdown,
/// its reasoning, its tool calls and the app's notices.
///
/// It follows the latest item as the agent streams, until the user scrolls up. Then a
/// "Jump to latest" pill, tagged `jump-to-latest`, scrolls back down and follows again.
pub(crate) struct Transcript {
    thread: Entity<Thread>,
    list: ListState,
    /// The conversation the list rows were last synced to, with each row's item revision then.
    synced: Option<ConversationId>,
    revisions: Vec<Revision>,
    /// Each reply's Markdown, parsed as it streams in so that a new chunk reparses only the
    /// blocks it touches.
    markdown: HashMap<ConversationItemId, StreamedMarkdown>,
    /// The tool calls and reasoning the user unfolded.
    expanded: HashSet<ConversationItemId>,
    highlighter: SyntaxHighlighter,
    following: bool,
}

impl Transcript {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        // Bottom-aligned so that a short conversation sits just above the composer.
        let list = ListState::new(0, ListAlignment::Bottom, px(OVERDRAW));
        list.set_follow_mode(FollowMode::Tail);
        let this = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            let following = event.is_following_tail;
            let _ = this.update(cx, |this, cx| {
                if this.following != following {
                    this.following = following;
                    cx.notify();
                }
            });
        });
        Self {
            thread,
            list,
            synced: None,
            revisions: Vec::new(),
            markdown: HashMap::new(),
            expanded: HashSet::new(),
            highlighter: SyntaxHighlighter::default(),
            following: true,
        }
    }

    /// Tells the list which rows were added, changed or replaced since it last rendered.
    fn sync(&mut self, conversation: Option<&Conversation>) {
        let id = conversation.map(Conversation::id);
        let revisions: Vec<Revision> = conversation
            .map(|conversation| conversation.items().iter().map(ConversationItem::revision).collect())
            .unwrap_or_default();
        if id != self.synced || revisions.len() < self.revisions.len() {
            self.list.reset(revisions.len());
            self.markdown.clear();
            self.expanded.clear();
        } else {
            for (index, (old, new)) in self.revisions.iter().zip(&revisions).enumerate() {
                if old != new {
                    self.list.remeasure_items(index..index + 1);
                }
            }
            self.list.splice(self.revisions.len()..self.revisions.len(), revisions.len() - self.revisions.len());
        }
        self.synced = id;
        self.revisions = revisions;
    }

    /// Folds or unfolds the item at `index`.
    fn toggle(&mut self, id: ConversationItemId, index: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        // Unfolding a row the list follows would otherwise scroll its top out of view.
        self.list.pause_following_tail();
        self.list.remeasure_items(index..index + 1);
        cx.notify();
    }

    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        self.following = true;
        cx.notify();
    }

    fn render_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let desktop_theme = DesktopTheme::global(cx);
        let theme = desktop_theme.ui();
        let thread = self.thread.read(cx);
        let Some(item) = thread.core().and_then(|core| core.conversation().items().get(index)) else {
            return div().into_any_element();
        };
        let id = item.id();
        let expanded = self.expanded.contains(&id);
        let toggle = cx.listener(move |this, _, _, cx| this.toggle(id, index, cx));
        let content = match item.content() {
            ConversationContent::User(_) => {
                Surface::new(theme).child(item.text().unwrap_or_default().into_owned()).into_any_element()
            }
            ConversationContent::Assistant(_) => {
                let markdown = self.markdown.entry(id).or_default();
                markdown.sync(&item.text().unwrap_or_default(), !item.is_open());
                MarkdownRenderer::new(index.to_string(), desktop_theme.review(), &theme, &mut self.highlighter)
                    .render(markdown.document())
            }
            ConversationContent::Thought(_) => {
                let text = item.text().unwrap_or_default().into_owned();
                thinking_block(text, item.is_open(), index, expanded, &theme, toggle)
            }
            ConversationContent::Tool(tool) => tool_card(tool, index, expanded, &theme, toggle),
            ConversationContent::Notice(text) => notice(text, &theme),
        };
        div()
            .debug_selector(move || format!("transcript-row-{index}"))
            .w_full()
            .px_4()
            .py_2()
            .child(content)
            .into_any_element()
    }
}

impl Render for Transcript {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let thread = self.thread.clone();
        let thread = thread.read(cx);
        let conversation = thread.core().map(SessionCore::conversation);
        self.sync(conversation);
        let transcript = div().debug_selector(|| "transcript".to_owned()).relative().flex_1().min_h_0().w_full();
        if conversation.is_none_or(|conversation| conversation.items().is_empty()) {
            return transcript.child(empty_state(&thread.status(), &theme));
        }
        let jump = (!self.following).then(|| {
            div().absolute().bottom_3().left_0().right_0().flex().justify_center().child(
                Button::new("jump-to-latest", "Jump to latest ↓", theme)
                    .variant(ButtonVariant::Primary)
                    .size(ControlSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx))),
            )
        });
        transcript
            .child(list(self.list.clone(), cx.processor(|this, index, _, cx| this.render_row(index, cx))).size_full())
            .children(jump)
    }
}

/// Rows measured beyond the viewport, so scrolling does not reveal unmeasured rows.
const OVERDRAW: f32 = 400.0;

fn empty_state(status: &ThreadStatus, theme: &UiTheme) -> EmptyState {
    match status {
        ThreadStatus::Connecting => {
            EmptyState::new("Connecting…", "Starting the agent for this thread", NoticeTone::Neutral, *theme)
        }
        ThreadStatus::Errored(error) => {
            EmptyState::new("The agent could not start", error.clone(), NoticeTone::Error, *theme)
        }
        ThreadStatus::Disconnected => {
            EmptyState::new("Disconnected", "The agent connection closed", NoticeTone::Warning, *theme)
        }
        ThreadStatus::Dormant => {
            EmptyState::new("Stopped", "This thread's agent is not running", NoticeTone::Neutral, *theme)
        }
        ThreadStatus::Idle | ThreadStatus::Running | ThreadStatus::NeedsInput => {
            EmptyState::new("New thread", "Send a prompt to start", NoticeTone::Neutral, *theme)
        }
    }
}
