use super::code::SharedHighlighter;
use super::markdown::code_actions;
use super::notice::notice;
use super::thinking::thinking_block;
use super::tool_card::ToolRenderer;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::UiTheme;
use crate::ui::tokens::{CONTENT_MAX_WIDTH, rem};
use crate::ui::{Button, ButtonVariant, EmptyState, IconName, ListScrollbar, NoticeTone, SurfaceExt};
use acp_utils::conversation::{
    Conversation, ConversationContent, ConversationId, ConversationItem, ConversationItemId, Revision,
};
use clankerdiff_gpui::DiffPreviewCache;
use client_core::session_core::SessionCore;
use gpui::{
    AnyElement, App, Context, Entity, FollowMode, ListAlignment, ListState, Subscription, Window, div, list,
    prelude::*, px,
};
use gpui_base::{TextView, TextViewState};
use std::collections::{HashMap, HashSet};

/// A thread's conversation, one list row per item: prompts, the agent's replies as Markdown,
/// its reasoning, its tool calls and the app's notices.
///
/// It follows the latest item as the agent streams, until the user scrolls up. Then a
/// "Jump to latest" pill scrolls back down and follows again.
pub(crate) struct Transcript {
    thread: Entity<Thread>,
    list: ListState,
    /// The conversation the list rows were last synced to, with each row's item revision then.
    synced: Option<ConversationId>,
    revisions: Vec<Revision>,
    replies: HashMap<ConversationItemId, Reply>,
    /// The tool calls and reasoning the user unfolded.
    expanded: HashSet<ConversationItemId>,
    highlighter: SharedHighlighter,
    patches: DiffPreviewCache,
    _thread_changed: Subscription,
}

impl Transcript {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        // Bottom-aligned so that a short conversation sits just above the composer.
        let list = ListState::new(0, ListAlignment::Bottom, px(OVERDRAW));
        list.set_follow_mode(FollowMode::Tail);
        let this = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            let _ = this.update(cx, |_, cx| cx.notify());
        });
        let thread_changed = cx.observe(&thread, |_, _, cx| cx.notify());
        Self {
            thread,
            list,
            synced: None,
            revisions: Vec::new(),
            replies: HashMap::new(),
            expanded: HashSet::new(),
            highlighter: SharedHighlighter::global(cx),
            patches: DiffPreviewCache::default(),
            _thread_changed: thread_changed,
        }
    }

    /// Tells the list which rows were added, changed or replaced since it last rendered, and
    /// returns the text of the replies among them.
    fn sync(&mut self, cx: &App) -> Vec<(usize, ConversationItemId, String)> {
        let conversation = self.thread.read(cx).core().map(SessionCore::conversation);
        let id = conversation.map(Conversation::id);
        let items = conversation.map(Conversation::items).unwrap_or_default();
        let revisions: Vec<Revision> = items.iter().map(ConversationItem::revision).collect();
        let changed: Vec<usize> = if id != self.synced || revisions.len() < self.revisions.len() {
            self.list.reset(revisions.len());
            self.replies.clear();
            self.patches.clear();
            self.expanded.clear();
            (0..revisions.len()).collect()
        } else {
            let changed: Vec<usize> =
                (0..revisions.len()).filter(|&index| self.revisions.get(index) != Some(&revisions[index])).collect();
            for &index in changed.iter().take_while(|&&index| index < self.revisions.len()) {
                self.list.remeasure_items(index..index + 1);
            }
            self.list.splice(self.revisions.len()..self.revisions.len(), revisions.len() - self.revisions.len());
            changed
        };
        self.synced = id;
        self.revisions = revisions;
        changed
            .into_iter()
            .filter_map(|index| {
                let item = &items[index];
                matches!(item.content(), ConversationContent::Assistant(_))
                    .then(|| (index, item.id(), item.text().unwrap_or_default().into_owned()))
            })
            .collect()
    }

    /// Hands a reply's latest text to its Markdown view, which parses it in the background. The
    /// row is remeasured whenever a parse lands, since that is when its height changes.
    fn update_reply(&mut self, index: usize, id: ConversationItemId, text: String, cx: &mut Context<Self>) {
        if let Some(reply) = self.replies.get_mut(&id) {
            match text.strip_prefix(reply.text.as_str()) {
                Some("") => {}
                Some(appended) => reply.view.update(cx, |view, cx| view.push_str(appended, cx)),
                None => reply.view.update(cx, |view, cx| view.set_text(&text, cx)),
            }
            reply.text = text;
            return;
        }
        let view = cx.new(|cx| TextViewState::markdown(&text, cx));
        let parsed = cx.observe(&view, move |this, _, _| this.list.remeasure_items(index..index + 1));
        self.replies.insert(id, Reply { view, text, _parsed: parsed });
    }

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
        let is_tool = matches!(item.content(), ConversationContent::Tool(_));
        let content = match item.content() {
            ConversationContent::User(_) => div()
                .px_3()
                .py_2()
                .surface(theme)
                .child(item.text().unwrap_or_default().into_owned())
                .into_any_element(),
            ConversationContent::Assistant(_) => match self.replies.get(&id) {
                Some(reply) => TextView::new(&reply.view)
                    .code_block_actions(move |block, _, _| code_actions(index, block, &theme))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            ConversationContent::Thought(_) => {
                let text = item.text().unwrap_or_default().into_owned();
                thinking_block(text, item.is_open(), index, expanded, &theme, toggle)
            }
            ConversationContent::Tool(tool) => self.highlighter.with(|highlighter| {
                ToolRenderer::new(desktop_theme.review(), highlighter, &mut self.patches)
                    .render(tool, index, expanded, toggle)
            }),
            ConversationContent::Notice(text) => notice(text, &theme),
        };
        div()
            .debug_selector(move || format!("transcript-row-{index}"))
            .w_full()
            .px_4()
            .when(!is_tool, Styled::py_2)
            .child(div().w_full().max_w(rem(CONTENT_MAX_WIDTH)).mx_auto().child(content))
            .into_any_element()
    }
}

impl Render for Transcript {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for (index, id, text) in self.sync(cx) {
            self.update_reply(index, id, text, cx);
        }
        let theme = DesktopTheme::global(cx).ui();
        let thread = self.thread.clone();
        let thread = thread.read(cx);
        let conversation = thread.core().map(SessionCore::conversation);
        let transcript = div().debug_selector(|| "transcript".to_owned()).relative().size_full();
        if conversation.is_none_or(|conversation| conversation.items().is_empty()) {
            return transcript.child(empty_state(&thread.status(), &theme));
        }
        let jump = (!self.list.is_following_tail()).then(|| {
            div().absolute().bottom_4().left_0().right_0().flex().justify_center().child(
                Button::new("jump-to-latest", "Jump to latest", theme)
                    .icon(IconName::ArrowDown)
                    .variant(ButtonVariant::Primary)
                    .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx))),
            )
        });
        let rows = list(self.list.clone(), cx.processor(|this, index, _, cx| this.render_row(index, cx))).size_full();
        transcript
            .flex()
            .child(div().relative().flex_1().min_w_0().h_full().child(rows).children(jump))
            .child(ListScrollbar::new("transcript-scrollbar", self.list.clone(), theme))
    }
}

/// Rows measured beyond the viewport, so scrolling does not reveal unmeasured rows.
const OVERDRAW: f32 = 400.0;

/// A reply's Markdown view and the text it was last handed.
struct Reply {
    view: Entity<TextViewState>,
    text: String,
    _parsed: Subscription,
}

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
