mod code;
mod groups;
mod markdown;
mod message;
mod tools;

pub(crate) use markdown::install as install_markdown;
pub(crate) use message::reconnect_banner;

use super::attachment::Attachment;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::tokens::{ROW_HEIGHT, rem};
use crate::ui::{EmptyState, Icon, IconName, ListScrollbar, NoticeTone, StyledExt, Tooltip, UiTheme};
use crate::ui::{Typography, TypographyExt};
use acp_utils::conversation::{
    Conversation, ConversationContent, ConversationId, ConversationItem, ConversationItemId, Notice, NoticeLevel,
    Revision,
};
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_core::DiffDocument;
use clankerdiff_gpui::{DiffPreviewCache, DiffViewer, DiffViewerEvent, DiffViewerOptions};
use client_core::session_core::SessionCore;
use code::SharedHighlighter;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FollowMode, Image, ListAlignment, ListState, Role, Subscription,
    Window, div, linear_color_stop, linear_gradient, list, prelude::*, px,
};
use gpui_base::{TextView, TextViewMotion, TextViewState};
use groups::{Group, GroupKind, groups};
use markdown::code_actions;
use message::{
    error, full_diff, image_preview, message_actions, notice, prompt, reconnect_button, row_group, thinking_block,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tools::{ToolActions, ToolFold, ToolRenderer, fold_header};

pub(crate) struct Transcript {
    thread: Entity<Thread>,
    list: ListState,
    synced: Option<ConversationId>,
    revisions: Vec<Revision>,
    groups: Vec<Group>,
    replies: HashMap<ConversationItemId, Reply>,
    attachments: HashMap<ConversationItemId, Arc<Vec<(usize, Attachment)>>>,
    unfolded: HashSet<Fold>,
    preview: Option<Arc<Image>>,
    full_diff: Option<(Entity<DiffViewer>, Subscription)>,
    highlighter: SharedHighlighter,
    patches: DiffPreviewCache,
    _thread_changed: Subscription,
}

impl Transcript {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        // The trailing row clears the fade; list padding would shrink the scrollbar range and stop drags reaching the end.
        let list = ListState::new(1, ListAlignment::Bottom, px(OVERDRAW));
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
            groups: Vec::new(),
            replies: HashMap::new(),
            attachments: HashMap::new(),
            unfolded: HashSet::new(),
            preview: None,
            full_diff: None,
            highlighter: SharedHighlighter::global(cx),
            patches: DiffPreviewCache::default(),
            _thread_changed: thread_changed,
        }
    }

    fn sync(&mut self, cx: &App) -> Vec<(usize, ConversationItemId, String)> {
        let conversation = self.thread.read(cx).core().map(SessionCore::conversation);
        let id = conversation.map(Conversation::id);
        let items = conversation.map(Conversation::items).unwrap_or_default();
        let revisions: Vec<Revision> = items.iter().map(ConversationItem::revision).collect();
        let groups = groups(items);
        let changed: Vec<usize> = if id != self.synced || revisions.len() < self.revisions.len() {
            self.list.reset(revisions.len() + 1);
            self.replies.clear();
            self.attachments.clear();
            self.patches.clear();
            self.unfolded.clear();
            self.full_diff = None;
            (0..revisions.len()).collect()
        } else {
            let changed: Vec<usize> =
                (0..revisions.len()).filter(|&index| self.revisions.get(index) != Some(&revisions[index])).collect();
            for &index in changed.iter().take_while(|&&index| index < self.revisions.len()) {
                self.list.remeasure_items(index..index + 1);
            }
            self.list.splice(self.revisions.len()..self.revisions.len(), revisions.len() - self.revisions.len());
            let regrouped = self.groups.iter().filter(|group| !groups.contains(group));
            for group in regrouped.chain(groups.iter().filter(|group| !self.groups.contains(group))) {
                self.list.remeasure_items(group.rows.clone());
            }
            changed
        };
        self.synced = id;
        self.revisions = revisions;
        self.groups = groups;
        changed
            .into_iter()
            .filter_map(|index| {
                let item = &items[index];
                matches!(item.content(), ConversationContent::Assistant(_))
                    .then(|| (index, item.id(), item.text().unwrap_or_default().into_owned()))
            })
            .collect()
    }

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

    fn toggle(&mut self, fold: Fold, rows: Range<usize>, cx: &mut Context<Self>) {
        if !self.unfolded.remove(&fold) {
            self.unfolded.insert(fold);
        }
        self.list.pause_following_tail();
        self.list.remeasure_items(rows);
        cx.notify();
    }

    fn open_full_diff(&mut self, document: &Arc<DiffDocument>, window: &mut Window, cx: &mut Context<Self>) {
        let theme = DesktopTheme::global(cx).review().clone();
        let options = DiffViewerOptions { themes: false, review_bar: false, ..DiffViewerOptions::default() };
        let viewer = cx.new(|_| DiffViewer::with_options(document.clone(), theme, options));
        let cancelled = cx.subscribe(&viewer, |this, _, event: &DiffViewerEvent, cx| {
            if matches!(event, DiffViewerEvent::Cancel) {
                this.close_full_diff(cx);
            }
        });
        self.full_diff = Some((viewer, cancelled));
        window.blur(cx);
        cx.notify();
    }

    fn close_full_diff(&mut self, cx: &mut Context<Self>) {
        self.full_diff = None;
        cx.notify();
    }

    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    fn render_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let desktop_theme = DesktopTheme::global(cx);
        let theme = desktop_theme.ui();
        let thread = self.thread.clone();
        let Some(items) = thread.read(cx).core().map(|core| core.conversation().items()) else {
            return div().into_any_element();
        };
        let Some(item) = items.get(index) else {
            return div().h(rem(FADE_HEIGHT)).into_any_element();
        };
        let group = self.groups.iter().find(|group| group.rows.contains(&index)).cloned();
        let (header, folded) = match &group {
            Some(Group { rows, kind }) => {
                let fold = Fold::Rows(*kind, items[rows.start].id());
                let expanded = self.unfolded.contains(&fold);
                let header = (index == rows.start).then(|| {
                    let toggle = cx.listener({
                        let rows = rows.clone();
                        move |this, _, _, cx| this.toggle(fold, rows.clone(), cx)
                    });
                    fold_header(*kind, index, &items[rows.clone()], expanded, &theme, toggle)
                });
                (header, !expanded)
            }
            None => (None, false),
        };
        if folded && header.is_none() {
            return div().into_any_element();
        }
        let content = (!folded).then(|| self.render_item(index, item, &theme, cx));
        let row_group = row_group(index);
        let is_tool = matches!(item.content(), ConversationContent::Tool(_));
        let is_prompt = matches!(item.content(), ConversationContent::User(_));
        div()
            .debug_selector(move || format!("transcript-row-{index}"))
            .group(row_group)
            .w_full()
            .px_4()
            .when(is_prompt, |row| row.pt_6().pb_2())
            .when(!is_tool && !is_prompt && group.is_none(), Styled::py_2)
            .when(header.is_some() && group.as_ref().is_some_and(|group| group.kind == GroupKind::Turn), Styled::py_2)
            .child(div().content_width().children(header).children(content.map(|content| {
                let indented = group.as_ref().is_some_and(|group| group.kind == GroupKind::Run);
                div().when(indented, Styled::pl_4).child(content)
            })))
            .into_any_element()
    }

    fn render_item(
        &mut self,
        index: usize,
        item: &ConversationItem,
        theme: &UiTheme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let fold = Fold::Item(item.id());
        let id = item.id();
        let expanded = self.unfolded.contains(&fold);
        let toggle = cx.listener(move |this, _, _, cx| this.toggle(fold, index..index + 1, cx));
        let copy = |text: String| message_actions(index, text, theme);
        match item.content() {
            ConversationContent::User(blocks) => {
                let text: String = blocks
                    .iter()
                    .filter_map(|block| match block {
                        acp::ContentBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect();
                let attachments = self
                    .attachments
                    .entry(id)
                    .or_insert_with(|| {
                        Arc::new(
                            blocks
                                .iter()
                                .enumerate()
                                .filter_map(|(block, content)| Some((block, Attachment::of(content)?)))
                                .collect(),
                        )
                    })
                    .clone();
                let open = cx.listener(|this, image: &Arc<Image>, _, cx| {
                    this.preview = Some(image.clone());
                    cx.notify();
                });
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(prompt(index, text.clone(), &attachments, theme, open))
                    .child(copy(text))
                    .into_any_element()
            }
            ConversationContent::Assistant(_) => match self.replies.get(&id) {
                Some(reply) => div()
                    .typography(Typography::Reading)
                    .child(div().debug_selector(move || format!("reply-{index}")).child(
                        TextView::new(&reply.view).motion(reply_motion()).code_block_actions({
                            let theme = *theme;
                            move |block, _, _| code_actions(index, block, &theme)
                        }),
                    ))
                    .when(!item.is_open(), |reply_row| reply_row.child(copy(reply.text.clone())))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            ConversationContent::Thought(_) => {
                let text = item.text().unwrap_or_default().into_owned();
                thinking_block(text, item.is_open(), index, expanded, theme, toggle)
            }
            ConversationContent::Tool(tool) => {
                let full = Fold::Full(id);
                let fold = match (expanded, self.unfolded.contains(&full)) {
                    (false, _) => ToolFold::Folded,
                    (true, false) => ToolFold::Expanded,
                    (true, true) => ToolFold::Full,
                };
                let show_all = cx.listener(move |this, _, _, cx| this.toggle(full, index..index + 1, cx));
                let actions = ToolActions {
                    open_diff: Rc::new(cx.listener(|this, document: &Arc<DiffDocument>, window, cx| {
                        this.open_full_diff(document, window, cx);
                    })),
                    open_location: Rc::new(cx.listener(|_, location: &acp::ToolCallLocation, _, cx| {
                        cx.emit(location.clone());
                    })),
                };
                self.highlighter.with(|highlighter| {
                    ToolRenderer::new(DesktopTheme::global(cx), highlighter, &mut self.patches, actions)
                        .render(tool, index, fold, toggle, show_all)
                })
            }
            ConversationContent::Notice(Notice { level: NoticeLevel::Info, text }) => notice(text, theme),
            ConversationContent::Notice(Notice { level: NoticeLevel::Error, text }) => error(index, text, theme),
        }
    }
}

impl EventEmitter<acp::ToolCallLocation> for Transcript {}

impl Render for Transcript {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for (index, id, text) in self.sync(cx) {
            self.update_reply(index, id, text, cx);
        }
        let theme = DesktopTheme::global(cx).ui();
        let thread = self.thread.clone();
        let thread = thread.read(cx);
        let status = thread.status();
        let conversation = thread.core().map(SessionCore::conversation);
        let body = if conversation.is_none_or(|conversation| conversation.items().is_empty()) {
            empty_state(&self.thread, status, thread.error(), &theme)
        } else {
            let jump = (!self.list.is_following_tail()).then(|| {
                div()
                    .id("jump-to-latest")
                    .debug_selector(|| "jump-to-latest".to_owned())
                    .role(Role::Button)
                    .aria_label("Jump to latest")
                    .flex_none()
                    .h(rem(ROW_HEIGHT))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(theme.colors.border_subtle)
                    .bg(theme.colors.elevated)
                    .shadow_md()
                    .text_color(theme.colors.text_secondary)
                    .cursor_pointer()
                    .hover(move |pill| pill.border_color(theme.colors.border).text_color(theme.colors.text))
                    .tooltip(Tooltip::text("Jump to latest", theme))
                    .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx)))
                    .child(Icon::new(IconName::ArrowDown, theme.colors.text_secondary))
            });
            let rows =
                list(self.list.clone(), cx.processor(|this, index, _, cx| this.render_row(index, cx))).size_full();
            let fade = linear_gradient(
                180.0,
                linear_color_stop(theme.colors.canvas.opacity(0.0), 0.0),
                linear_color_stop(theme.colors.canvas, FADE_OPAQUE_FROM),
            );
            let foot = div().absolute().bottom_0().left_0().right_0().h(rem(FADE_HEIGHT)).bg(fade);
            let jump = jump.map(|jump| {
                div().absolute().bottom(rem(FADE_HEIGHT)).left_0().right_0().flex().justify_center().child(jump)
            });
            div()
                .size_full()
                .flex()
                .child(div().relative().flex_1().min_w_0().h_full().child(rows).child(foot).children(jump))
                .child(ListScrollbar::new("transcript-scrollbar", self.list.clone(), theme))
                .into_any_element()
        };
        let full_diff = self.full_diff.as_ref().map(|(viewer, _)| {
            let this = cx.entity().downgrade();
            full_diff(viewer.clone(), &theme, move |_, cx| {
                let _ = this.update(cx, Self::close_full_diff);
            })
        });
        let preview = self.preview.clone().map(|image| {
            let this = cx.entity().downgrade();
            image_preview(image, &theme, move |_, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.preview = None;
                    cx.notify();
                });
            })
        });
        div()
            .debug_selector(|| "transcript".to_owned())
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(preview)
            .children(full_diff)
    }
}

const FADE_HEIGHT: f32 = 24.0;

const FADE_OPAQUE_FROM: f32 = 0.6;

const OVERDRAW: f32 = 400.0;

fn reply_motion() -> TextViewMotion {
    TextViewMotion::default()
        .with_stream_fade(Duration::from_millis(STREAM_FADE_MILLIS))
        .with_stream_fade_stagger(Duration::from_millis(STREAM_FADE_STAGGER_MILLIS))
}

const STREAM_FADE_MILLIS: u64 = 240;
const STREAM_FADE_STAGGER_MILLIS: u64 = 12;

struct Reply {
    view: Entity<TextViewState>,
    text: String,
    _parsed: Subscription,
}

fn empty_state(thread: &Entity<Thread>, status: ThreadStatus, error: Option<&str>, theme: &UiTheme) -> AnyElement {
    if let Some(problem) = status.problem() {
        let detail = error.unwrap_or("Reconnect to pick up where it left off").to_owned();
        let tone = if status == ThreadStatus::Errored { NoticeTone::Error } else { NoticeTone::Warning };
        return div()
            .size_full()
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .text_center()
            .child(div().typography(Typography::Heading).text_color(theme.notice_style(tone).foreground).child(problem))
            .child(div().typography(Typography::Body).text_color(theme.colors.text_muted).child(detail))
            .child(reconnect_button(thread, theme))
            .into_any_element();
    }
    let (title, detail) = match status {
        ThreadStatus::Connecting => ("Connecting…", "Starting the agent for this thread"),
        ThreadStatus::Dormant => ("Stopped", "This thread's agent is not running"),
        _ => ("New thread", "Send a prompt to start"),
    };
    EmptyState::new(title, detail, NoticeTone::Neutral, *theme).into_any_element()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Fold {
    Item(ConversationItemId),
    Full(ConversationItemId),
    Rows(GroupKind, ConversationItemId),
}
