mod code;
mod groups;
mod markdown;
mod message;
mod tools;

pub(crate) use message::reconnect_banner;

use super::attachment::Attachment;
use crate::platform::Platform;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::tokens::ROW_HEIGHT;
use crate::ui::{ActiveTheme, Hint, Icon, IconName, ListScrollbar, StyledExt, UiStyles, UiTheme, empty_state};
use acp_utils::conversation::{
    ConversationContent, ConversationId, ConversationItem, ConversationItemId, Notice, NoticeLevel, Revision,
};
use agent_client_protocol::v2::schema as acp;
use clankerdiff_core::DiffDocument;
use clankerdiff_rouge::{DiffViewerEvent, DiffViewerOptions, ReviewController, ReviewHandle, ReviewOutput};
use clankerdiff_rouge_shell::rouge::core::{
    Align, AnyView, BoxShadow, Cursor, FocusHandle, RowKey, Styled, TextAlign, View, VirtualListHandle, WindowHandle,
    div, dynamic, focus_handle, px, root_font_size, show, text, view, virtual_list, virtual_list_handle, window_handle,
};
use clankerdiff_rouge_shell::rouge::reactive::{
    Callback, Memo, Scope, Signal, Stored, effect, memo, signal, stored, untrack,
};
use groups::{Group, GroupKind, groups};
use message::{error, full_diff, image_preview, message_actions, notice, prompt, reconnect_button, thinking_block};
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use tools::{ToolActions, ToolCard, ToolFold, ToolPreviews, fold_header};

#[derive(Clone, Copy)]
pub(crate) struct Transcript {
    thread: Thread,
    list: VirtualListHandle,
    following: Signal<bool>,
    unfolded: Signal<HashSet<Fold>>,
    preview: Signal<Option<Arc<[u8]>>>,
    full_diff: Signal<Option<ReviewHandle>>,
    attachments: Stored<HashMap<ConversationItemId, Rc<ItemAttachments>>>,
    tools: Stored<ToolPreviews>,
    focus: Stored<HashMap<ConversationItemId, FocusHandle>>,
    summary: Memo<Summary>,
    structure: Memo<u64>,
    on_location: Callback<(acp::ToolCallLocation,)>,
    platform: Stored<Rc<dyn Platform>>,
    window: WindowHandle,
    scope: Scope,
}

type ItemAttachments = Vec<(usize, Attachment)>;

#[derive(Clone, Default, PartialEq)]
struct Summary {
    conversation: Option<ConversationId>,
    items: Vec<(ConversationItemId, Revision)>,
    groups: Vec<Group>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Fold {
    Item(ConversationItemId),
    Full(ConversationItemId),
    Rows(GroupKind, ConversationItemId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RowId {
    Item(ConversationItemId),
    Spacer,
}

impl Transcript {
    pub(crate) fn new(
        thread: Thread,
        platform: Rc<dyn Platform>,
        on_location: impl Fn(acp::ToolCallLocation) + 'static,
    ) -> Self {
        let summary = memo(move || {
            thread
                .with_core(|core| {
                    let conversation = core.conversation();
                    let items = conversation.items();
                    Summary {
                        conversation: Some(conversation.id()),
                        items: items.iter().map(|item| (item.id(), item.revision())).collect(),
                        groups: groups(items),
                    }
                })
                .unwrap_or_default()
        });
        let unfolded: Signal<HashSet<Fold>> = signal(HashSet::new());
        let structure = memo(move || {
            let groups = summary.with(|summary| hash(&(summary.conversation, &summary.groups)));
            let folds = unfolded.with(|unfolded| unfolded.iter().map(hash).fold(0, u64::wrapping_add));
            groups ^ folds
        });
        let transcript = Self {
            thread,
            list: virtual_list_handle(),
            following: signal(true),
            unfolded,
            preview: signal(None),
            full_diff: signal(None),
            attachments: stored(HashMap::new()),
            tools: stored(ToolPreviews::default()),
            focus: stored(HashMap::new()),
            summary,
            structure,
            on_location: Callback::from(on_location),
            platform: stored(platform),
            window: window_handle(),
            scope: Scope::new(),
        };
        transcript.observe();
        transcript
    }

    fn observe(self) {
        let summary = self.summary;
        let previous: Cell<(Option<ConversationId>, usize)> = Cell::new((None, 0));
        effect(move || {
            let current = summary.with(|summary| (summary.conversation, summary.items.len()));
            let (was, had) = previous.replace(current);
            if was != current.0 || current.1 < had {
                untrack(|| {
                    self.attachments.update(HashMap::clear);
                    self.tools.update(ToolPreviews::clear);
                    self.focus.update(HashMap::clear);
                    self.unfolded.update(HashSet::clear);
                    self.full_diff.set(None);
                });
            }
        });
        effect(move || {
            let rows = summary.with(|summary| summary.items.clone());
            untrack(|| {
                if self.following.get_untracked() {
                    self.list.scroll_to_row(rows.len(), Align::Bottom);
                }
            });
        });
        let offset = Cell::new(0.0_f32);
        let state = self.list.scroll_state();
        effect(move || {
            let state = state.get();
            let at = state.offset.y;
            let bottom = at >= state.max_offset().y - FOLLOW_SLOP;
            let up = at < offset.replace(at) - FOLLOW_SLOP;
            untrack(|| {
                if bottom {
                    self.following.set_if_changed(true);
                } else if up {
                    self.following.set_if_changed(false);
                }
            });
        });
    }

    pub(crate) fn view(self) -> impl View {
        let empty = memo(move || self.summary.with(|summary| summary.items.is_empty()));
        let close_preview = Callback::from(move || self.preview.set(None));
        let close_diff = Callback::from(move || self.full_diff.set(None));
        view! {
            <div test_id="transcript" relative size_full flex col>
                <div flex_1 min_h=0.0 w_full>
                    <dynamic>{if empty.get() { self.empty_state().into_any() } else { untrack(|| self.list()).into_any() }}</dynamic>
                </div>
                <dynamic>{self.preview.get().map(|bytes| image_preview(bytes, close_preview))}</dynamic>
                <dynamic>{self.full_diff.get().map(|handle| full_diff(&handle, close_diff))}</dynamic>
            </div>
        }
    }

    fn list(self) -> impl View {
        let theme = UiTheme::current();
        let (summary, structure, list) = (self.summary, self.structure, self.list);
        let rows = virtual_list(
            move || summary.with(|summary| summary.items.len()) + 1,
            ROW_ESTIMATE,
            move |index| untrack(|| self.row(index)),
        )
        .row_identity(move |index| {
            let id = summary.with(|summary| summary.items.get(index).map_or(RowId::Spacer, |(id, _)| RowId::Item(*id)));
            RowKey::new(id, structure.get())
        })
        .measure_rows(|| u64::from(root_font_size().get().to_bits()))
        .handle(list)
        .size_full();
        let following = self.following;
        let scroll = list.scroll_state();
        let jump = move || {
            view! {
            <div test_id="jump-to-latest" flex_none h=ROW_HEIGHT px=12.0 flex items_center rounded={ROW_HEIGHT / 2.0}
                 border=px(1.0) border_color=theme.border_subtle bg=theme.elevated shadow=BoxShadow::md()
                 cursor=Cursor::Pointer
                 on_click={move |event| {
                     event.stop_propagation();
                     following.set(true);
                     list.scroll_to_row(summary.with_untracked(|summary| summary.items.len()), Align::Bottom);
                 }}>
                {Icon::new(IconName::ArrowDown, theme.text_secondary)}
            </div>
            }
            .tooltip("Jump to latest")
        };
        view! {
            <div size_full flex>
                <div relative flex_1 min_w=0.0 h_full>
                    {rows}
                    <show when={move || !following.get()}>
                        <div absolute bottom=FADE_HEIGHT left=0.0 right=0.0 flex justify_center>{jump()}</div>
                    </show>
                </div>
                <show when={move || scroll.with(|scroll| scroll.content.height > scroll.viewport.height + 0.5)}>
                    <div test_id="transcript-scrollbar" flex_none h_full>{ListScrollbar::list(list)}</div>
                </show>
            </div>
        }
    }

    fn row(self, index: usize) -> AnyView {
        let (item, group) = self.summary.with(|summary| {
            let group = summary.groups.iter().find(|group| group.rows.contains(&index)).cloned();
            (summary.items.get(index).map(|(id, _)| *id), group)
        });
        if item.is_none() {
            return view! { <div h=FADE_HEIGHT/> }.into_any();
        }
        let Some((kind, header)) = self
            .thread
            .with_core(|core| {
                let items = core.conversation().items();
                let kind = items.get(index).map(kind_of)?;
                let header = group.as_ref().map(|Group { rows, kind }| {
                    let fold = Fold::Rows(*kind, items[rows.start].id());
                    let expanded = self.unfolded.with(|unfolded| unfolded.contains(&fold));
                    let toggle = Callback::from(move || self.toggle(fold));
                    let header = (index == rows.start)
                        .then(|| fold_header(*kind, index, &items[rows.clone()], expanded, toggle));
                    (header, !expanded)
                });
                Some((kind, header))
            })
            .flatten()
        else {
            return view! { <div h=px(1.0)/> }.into_any();
        };
        let (header, folded) = header.unwrap_or((None, false));
        if folded && header.is_none() {
            return view! { <div h=px(1.0)/> }.into_any();
        }
        let hovered = signal(false);
        let content = (!folded).then(|| {
            let indented = group.as_ref().is_some_and(|group| group.kind == GroupKind::Run);
            let item = self.item(index, kind, hovered);
            if indented { div().pl(16.0).child(item) } else { div().child(item) }
        });
        let turn_header = header.is_some() && group.as_ref().is_some_and(|group| group.kind == GroupKind::Turn);
        let padding = match kind {
            Kind::User => (24.0, 8.0),
            Kind::Tool if !turn_header => (0.0, 0.0),
            _ if group.is_some() && !turn_header => (0.0, 0.0),
            _ => (8.0, 8.0),
        };
        view! {
            <div test_id={format!("transcript-row-{index}")} w_full px=16.0 pt=padding.0 pb=padding.1
                 on_hover={move |inside| {
                     hovered.set_if_changed(inside);
                 }}>
                <div content_width>{header}{content}</div>
            </div>
        }
        .into_any()
    }

    fn item(self, index: usize, kind: Kind, hovered: Signal<bool>) -> AnyView {
        let thread = self.thread;
        match kind {
            Kind::User => self.user(index, hovered),
            Kind::Assistant => {
                let text = memo(move || {
                    thread
                        .with_core(|core| {
                            core.conversation().items().get(index).and_then(|item| item.text().map(Cow::into_owned))
                        })
                        .flatten()
                        .unwrap_or_default()
                });
                let open = memo(move || {
                    thread
                        .with_core(|core| core.conversation().items().get(index).is_some_and(ConversationItem::is_open))
                        .unwrap_or(false)
                });
                let (window, platform) = (self.window, self.platform);
                view! {
                    <div reading>
                        <div test_id={format!("reply-{index}")}>{markdown::reply(text, window, platform)}</div>
                        <dynamic>{(!open.get()).then(|| message_actions(index, text.get_untracked(), hovered, window))}</dynamic>
                    </div>
                }
                .into_any()
            }
            Kind::Thought | Kind::Tool | Kind::Notice => {
                let revision = memo(move || {
                    thread
                        .with_core(|core| core.conversation().items().get(index).map(ConversationItem::revision))
                        .flatten()
                });
                dynamic(move || {
                    revision.get();
                    untrack(|| self.settled_item(index))
                })
                .into_any()
            }
        }
    }

    fn user(self, index: usize, hovered: Signal<bool>) -> AnyView {
        let Some((id, text, blocks)) = self
            .thread
            .with_core(|core| {
                let item = core.conversation().items().get(index)?;
                let ConversationContent::User(blocks) = item.content() else {
                    return None;
                };
                let text: String = blocks
                    .iter()
                    .filter_map(|block| match block {
                        acp::ContentBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect();
                Some((item.id(), text, blocks.clone()))
            })
            .flatten()
        else {
            return ().into_any();
        };
        let attachments = self.attachments.update(|attachments| {
            attachments
                .entry(id)
                .or_insert_with(|| {
                    Rc::new(
                        blocks
                            .iter()
                            .enumerate()
                            .filter_map(|(block, content)| Some((block, Attachment::of(content)?)))
                            .collect(),
                    )
                })
                .clone()
        });
        let open = Callback::from(move |bytes: Arc<[u8]>| self.preview.set(Some(bytes)));
        view! {
            <div flex col items_end>
                {prompt(index, text.clone(), &attachments, open)}
                {message_actions(index, text, hovered, self.window)}
            </div>
        }
        .into_any()
    }

    fn settled_item(self, index: usize) -> AnyView {
        let theme = ActiveTheme::current();
        let review = theme.review();
        let actions = ToolActions {
            open_diff: Callback::from(move |document: Arc<DiffDocument>| self.open_full_diff(document)),
            open_location: self.on_location,
        };
        self.thread
            .with_core(|core| {
                let item = core.conversation().items().get(index)?;
                let id = item.id();
                let expanded = self.unfolded.with(|unfolded| unfolded.contains(&Fold::Item(id)));
                let toggle = Callback::from(move || self.toggle(Fold::Item(id)));
                Some(match item.content() {
                    ConversationContent::Thought(_) => {
                        let text = item.text().unwrap_or_default().into_owned();
                        thinking_block(text, item.is_open(), index, expanded, toggle)
                    }
                    ConversationContent::Tool(tool) => {
                        let full = Fold::Full(id);
                        let fold = match (expanded, self.unfolded.with(|unfolded| unfolded.contains(&full))) {
                            (false, _) => ToolFold::Folded,
                            (true, false) => ToolFold::Expanded,
                            (true, true) => ToolFold::Full,
                        };
                        let show_all = Callback::from(move || self.toggle(full));
                        let preview = self.tools.update(|tools| tools.get(item, tool));
                        let focus = self
                            .focus
                            .update(|handles| *handles.entry(id).or_insert_with(|| self.scope.run(focus_handle)));
                        let card =
                            ToolCard { review: &review, theme: theme.review_signal(), preview: &preview, actions };
                        card.render(tool, index, fold, focus, toggle, show_all)
                    }
                    ConversationContent::Notice(Notice { level: NoticeLevel::Info, text }) => notice(text),
                    ConversationContent::Notice(Notice { level: NoticeLevel::Error, text }) => error(index, text),
                    ConversationContent::User(_) | ConversationContent::Assistant(_) => ().into_any(),
                })
            })
            .flatten()
            .unwrap_or_else(|| ().into_any())
    }

    fn toggle(self, fold: Fold) {
        self.following.set(false);
        self.unfolded.update(|unfolded| {
            if !unfolded.remove(&fold) {
                unfolded.insert(fold);
            }
        });
    }

    fn open_full_diff(self, document: Arc<DiffDocument>) {
        let theme = ActiveTheme::current().review();
        let options = DiffViewerOptions { themes: false, review_bar: false, ..DiffViewerOptions::default() };
        let handle = self.scope.run(|| {
            ReviewController::with_options(document, theme, options).mount(move |output| {
                if output == ReviewOutput::Event(DiffViewerEvent::Cancel) {
                    self.full_diff.set(None);
                }
            })
        });
        self.full_diff.set(Some(handle));
    }

    fn empty_state(self) -> impl View {
        let thread = self.thread;
        let state = memo(move || (thread.status(), thread.error()));
        dynamic(move || {
            let (status, error) = state.get();
            if let Some(problem) = status.problem() {
                let theme = UiTheme::current();
                let detail = error.unwrap_or_else(|| "Reconnect to pick up where it left off".to_owned());
                let color = if status == ThreadStatus::Errored { theme.destructive } else { theme.warning };
                return view! {
                    <div size_full p=24.0 flex col items_center justify_center gap=12.0>
                        <text heading color=color align=TextAlign::Center>{problem}</text>
                        <text body muted align=TextAlign::Center>{detail}</text>
                        {reconnect_button(thread)}
                    </div>
                }
                .into_any();
            }
            let (title, detail) = match status {
                ThreadStatus::Connecting => ("Connecting…", "Starting the agent for this thread"),
                ThreadStatus::Dormant => ("Stopped", "This thread's agent is not running"),
                _ => ("New thread", "Send a prompt to start"),
            };
            view! {
                <div size_full flex items_center justify_center>
                    <empty_state icon=IconName::MessageSquareText title detail/>
                </div>
            }
            .into_any()
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Assistant,
    Thought,
    Tool,
    Notice,
}

fn kind_of(item: &ConversationItem) -> Kind {
    match item.content() {
        ConversationContent::User(_) => Kind::User,
        ConversationContent::Assistant(_) => Kind::Assistant,
        ConversationContent::Thought(_) => Kind::Thought,
        ConversationContent::Tool(_) => Kind::Tool,
        ConversationContent::Notice(_) => Kind::Notice,
    }
}

fn hash(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

const FADE_HEIGHT: f32 = 24.0;
const ROW_ESTIMATE: f32 = 48.0;
const FOLLOW_SLOP: f32 = 2.0;
