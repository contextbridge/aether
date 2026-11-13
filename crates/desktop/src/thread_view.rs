use crate::artifact_review::ArtifactReview;
use crate::chat::{Composer, SessionOptions, StatusLine, Transcript, reconnect_banner};
use crate::elicitation::ElicitationCard;
use crate::files::Files;
use crate::platform::Platform;
use crate::registry::Registry;
use crate::sidebar::toggle_button;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::tokens::{GROUP_HEIGHT, HEADER_HEIGHT, ICON_SIZE_SMALL, PANEL_INSET, ROW_HEIGHT};
use crate::ui::{GlassExt, Icon, IconName, StyledExt, UiStyles, UiTheme, scroll_edge};
use agent_client_protocol::v2::schema as acp;
use clankerdiff_core::RepoPath;
use clankerdiff_rouge_shell::rouge::core::{
    AnyView, Color, Cursor, Div, FileDropEvent, FileDropPhase, Rect, Styled, View, div, dynamic, show, text, view,
};
use clankerdiff_rouge_shell::rouge::reactive::{Callback, Memo, Signal, effect, memo, signal, untrack};
use std::rc::Rc;

#[derive(Clone, Copy)]
pub(crate) struct ThreadView {
    thread: Thread,
    registry: Registry,
    tab: Signal<ThreadTab>,
    changes_seen: Signal<Option<usize>>,
    dropping: Signal<bool>,
    dock: Signal<Rect>,
    transcript: Transcript,
    status: StatusLine,
    options: SessionOptions,
    elicitation: ElicitationCard,
    composer: Composer,
    files: Files,
    review: ArtifactReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadTab {
    Chat,
    Files,
    Review,
}

impl ThreadView {
    pub(crate) fn new(thread: Thread, registry: Registry, platform: Rc<dyn Platform>) -> Self {
        let tab = signal(ThreadTab::Chat);
        let changes_seen = signal(None);
        let show = move |next: ThreadTab| show_tab(thread, tab, changes_seen, next);
        let files = Files::new(thread, move || show(ThreadTab::Chat));
        let review = ArtifactReview::new(thread, platform.clone(), move || {
            if tab.get_untracked() == ThreadTab::Review {
                show(ThreadTab::Chat);
            }
        });
        let dock = signal(Rect::ZERO);
        let transcript = Transcript::new(thread, platform.clone(), dock, move |location: acp::ToolCallLocation| {
            open_location(thread, files, tab, changes_seen, &location);
        });
        let view = Self {
            thread,
            registry,
            tab,
            changes_seen,
            dropping: signal(false),
            dock,
            transcript,
            status: StatusLine::new(thread),
            options: SessionOptions::new(thread),
            elicitation: ElicitationCard::new(thread, platform.clone(), move || show(ThreadTab::Review)),
            composer: Composer::new(thread, platform),
            files,
            review,
        };
        thread.on_event(move |event| {
            if event == crate::threads::ThreadEvent::ReviewSubmitted {
                show(ThreadTab::Chat);
            }
        });
        let changed = memo(move || changed_files(thread));
        effect(move || {
            changed.get();
            untrack(|| see_changes(thread, tab, changes_seen));
        });
        view
    }

    pub(crate) fn files(self) -> Files {
        self.files
    }

    pub(crate) fn show(self, tab: ThreadTab) {
        show_tab(self.thread, self.tab, self.changes_seen, tab);
    }

    pub(crate) fn open_file(self, path: RepoPath, line: Option<u32>) {
        self.show(ThreadTab::Files);
        self.files.open(path, line);
    }

    pub(crate) fn view(self, sidebar_collapsed: Memo<bool>) -> impl View {
        let tab = self.tab;
        let chat = memo(move || tab.get() == ThreadTab::Chat);
        let body = self.body(chat);
        view! {
            <div relative size_full>
                {body}
                {self.header(sidebar_collapsed)}
            </div>
        }
    }

    fn header(self, sidebar_collapsed: Memo<bool>) -> impl View {
        let (thread, registry) = (self.thread, self.registry);
        let title = memo(move || registry.with(|state| state.title(thread.session_id().as_ref(), thread.title())));
        let project = thread.project().map(|project| project.display().to_string());
        let agent = thread.target().label().to_owned();
        let project = project.map(|project| {
            view! { <div min_w=0.0 overflow_hidden><text small muted no_wrap>{project}</text></div> }
        });
        view! {
            <div test_id="thread-header" absolute top=0.0 left=0.0 right=0.0 h=HEADER_HEIGHT pt=PANEL_INSET px=16.0
                 flex items_center gap=12.0 pass_through>
                {scroll_edge(true, PANEL_INSET, HEADER_HEIGHT)}
                <div flex_1 min_w=0.0 flex pass_through>
                    <div test_id="thread-heading" min_w=0.0 h=GROUP_HEIGHT px=16.0 flex items_center gap=8.0
                         rounded={GROUP_HEIGHT / 2.0} glass_control>
                        <dynamic>{sidebar_collapsed.get().then(|| toggle_button(false))}</dynamic>
                        <div test_id="thread-title" min_w=0.0 overflow_hidden>
                            <text title no_wrap>{move || title.get()}</text>
                        </div>
                        {project}
                        <div test_id="thread-agent" flex_none><text small muted>{agent}</text></div>
                    </div>
                </div>
                {self.switch()}
            </div>
        }
    }

    fn switch(self) -> impl View {
        let (thread, tab, seen) = (self.thread, self.tab, self.changes_seen);
        let theme = UiTheme::current();
        let files = memo(move || changed_files(thread));
        let working = memo(move || match thread.status() {
            _ if tab.get() == ThreadTab::Chat => None,
            ThreadStatus::NeedsInput => Some("chat-waiting"),
            ThreadStatus::Running | ThreadStatus::Background => Some("chat-working"),
            _ => None,
        });
        let unseen =
            memo(move || files.get().is_some_and(|files| tab.get() != ThreadTab::Files && seen.get() != Some(files)));
        let reviewing = memo(move || thread.has_pending_review());
        let chat = segment(&self, "tab-chat", IconName::MessageSquareText, "Chat", ThreadTab::Chat).child(dynamic(
            move || {
                working.get().map(|id| {
                    let color = if id == "chat-waiting" { theme.warning } else { theme.info };
                    corner_dot(id, color.get())
                })
            },
        ));
        let files_tab = segment(&self, "tab-files", IconName::Folder, "Files", ThreadTab::Files)
            .child(dynamic(move || {
                files.get().map(|files| {
                    view! { <div test_id="changes-count"><text muted>{files.to_string()}</text></div> }
                })
            }))
            .child(dynamic(move || unseen.get().then(|| corner_dot("changes-unseen", theme.accent.get()))));
        view! {
            <div test_id="view-switch" flex_none h=GROUP_HEIGHT p=4.0 flex items_center gap=4.0
                 rounded={GROUP_HEIGHT / 2.0} glass_control small>
                {chat}
                {files_tab}
                <dynamic>{reviewing.get().then(|| segment(&self, "tab-review", IconName::Eye, "Review", ThreadTab::Review))}</dynamic>
            </div>
        }
    }

    fn body(self, chat: Memo<bool>) -> impl View {
        let (tab, dropping, composer) = (self.tab, self.dropping, self.composer);
        let theme = UiTheme::current();
        let shown = memo(move || tab.get());
        let content = dynamic(move || match shown.get() {
            ThreadTab::Chat => untrack(|| self.transcript.view()).into_any(),
            ThreadTab::Files => below_header(untrack(|| self.files.view())),
            ThreadTab::Review => below_header(untrack(|| self.review.view())),
        });
        let drop = move |event: &FileDropEvent| {
            if !chat.get_untracked() {
                return;
            }
            event.stop_propagation();
            match event.phase {
                FileDropPhase::Hover => dropping.set(true),
                FileDropPhase::Leave => dropping.set(false),
                FileDropPhase::Drop => {
                    dropping.set(false);
                    composer.attach(&event.paths);
                    composer.focus();
                }
            }
        };
        view! {
            <div relative size_full on_file_drop=drop
                 bg={move || if dropping.get() { theme.accent.get().with_alpha(DROP_TINT) } else { Color::TRANSPARENT }}>
                {content}
                {self.review_pill()}
                <show when={move || chat.get()}>{self.dock()}</show>
            </div>
        }
    }

    fn dock(self) -> impl View {
        let thread = self.thread;
        let unreachable = memo(move || unreachable(thread));
        view! {
            <div absolute left=0.0 right=0.0 bottom=0.0 px=16.0 pb=12.0 pass_through track_bounds={self.dock}>
                {scroll_edge(false, DOCK_EDGE_SOLID, DOCK_EDGE_REACH)}
                <div content_width flex col>
                    <dynamic>{unreachable.get().and_then(|status| reconnect_banner(thread, status)).map(|banner| banner.mt(8.0))}</dynamic>
                    {self.elicitation.view()}
                    {self.composer.view()}
                    <div test_id="composer-bar" h=ROW_HEIGHT mt=4.0 px=8.0 flex items_center gap=16.0>
                        {self.options.view()}
                        {self.status.view()}
                    </div>
                </div>
            </div>
        }
    }

    fn review_pill(self) -> impl View {
        let (tab, files) = (self.tab, self.files);
        let comments = memo(move || if tab.get() == ThreadTab::Files { files.comments() } else { 0 });
        dynamic(move || {
            let comments = comments.get();
            (comments > 0).then(|| {
                let label = if comments == 1 { "1 comment".to_owned() } else { format!("{comments} comments") };
                view! {
                    <div absolute left=0.0 right=0.0 bottom=16.0 flex justify_center>
                        <div test_id="review-pill" h=GROUP_HEIGHT p=4.0 pl=16.0 flex items_center gap=4.0 glass
                             rounded={GROUP_HEIGHT / 2.0} small>
                            <div test_id="review-comments" mr=8.0><text no_wrap secondary>{label}</text></div>
                            {pill_button("review-copy", "Copy", false, Callback::from(move || files.copy()))}
                            {pill_button("review-submit", "Submit review", true, Callback::from(move || files.submit()))}
                        </div>
                    </div>
                }
            })
        })
    }
}

fn show_tab(thread: Thread, tab: Signal<ThreadTab>, seen: Signal<Option<usize>>, next: ThreadTab) {
    if next == ThreadTab::Review && !thread.has_pending_review() {
        return;
    }
    tab.set(next);
    see_changes(thread, tab, seen);
}

fn see_changes(thread: Thread, tab: Signal<ThreadTab>, seen: Signal<Option<usize>>) {
    if tab.get_untracked() == ThreadTab::Files {
        seen.set_if_changed(untrack(|| changed_files(thread)));
    }
}

fn open_location(
    thread: Thread,
    files: Files,
    tab: Signal<ThreadTab>,
    seen: Signal<Option<usize>>,
    location: &acp::ToolCallLocation,
) {
    let path = location.path.0.as_path();
    let Some(file) = thread.listed_files().ok().and_then(|files| files.repo_path(path)) else {
        let message = format!("Can't open {}: it isn't in this thread's repository", path.display());
        return thread.report_error(&message);
    };
    show_tab(thread, tab, seen, ThreadTab::Files);
    files.open(file, location.line.map(std::num::NonZeroU32::get));
}

fn segment(view: &ThreadView, id: &'static str, icon: IconName, label: &'static str, value: ThreadTab) -> Div {
    let (theme, view) = (UiTheme::current(), *view);
    let tab = view.tab;
    let selected = memo(move || tab.get() == value);
    let hovered = signal(false);
    let foreground = move || {
        if selected.get() || hovered.get() { theme.text.get() } else { theme.text_secondary.get() }
    };
    view! {
        <div test_id=id relative h_full px=12.0 flex items_center gap=4.0 rounded={ROW_HEIGHT / 2.0}
             cursor=Cursor::Pointer text_color=foreground
             bg={move || if selected.get() { theme.text.get().with_alpha(SELECTED_TINT) } else { Color::TRANSPARENT }}
             on_hover={move |inside| {
                 hovered.set_if_changed(inside);
             }}
             on_click={move |event| {
                 event.stop_propagation();
                 view.show(value);
             }}>
            <div test_id={format!("{id}-icon")} flex>{Icon::new(icon, theme.text_muted).size(ICON_SIZE_SMALL)}</div>
            <text>{label}</text>
        </div>
    }
}

fn below_header(content: impl View) -> AnyView {
    view! { <div size_full pt=HEADER_HEIGHT>{content}</div> }.into_any()
}

fn corner_dot(id: &'static str, color: Color) -> AnyView {
    view! {
        <div test_id=id absolute top=0.0 right=0.0 size=CORNER_DOT_SIZE_OFF_GRID rounded={CORNER_DOT_SIZE_OFF_GRID / 2.0}
             bg=color/>
    }
    .into_any()
}

fn pill_button(id: &'static str, label: &'static str, primary: bool, on_click: Callback) -> AnyView {
    let theme = UiTheme::current();
    let hovered = signal(false);
    view! {
        <div test_id=id h_full px=12.0 flex items_center rounded={ROW_HEIGHT / 2.0} cursor=Cursor::Pointer
             bg={move || match (primary, hovered.get()) {
                 (true, false) => theme.accent.get(),
                 (true, true) => theme.accent.get().with_alpha(PILL_HOVER_OPACITY),
                 (false, true) => theme.text.get().with_alpha(SELECTED_TINT),
                 (false, false) => Color::TRANSPARENT,
             }}
             on_hover={move |inside| {
                 hovered.set_if_changed(inside);
             }}
             on_click={move |event| {
                 event.stop_propagation();
                 on_click.call();
             }}>
            <text no_wrap color={move || if primary { theme.accent_foreground.get() } else { theme.text.get() }}>{label}</text>
        </div>
    }
    .into_any()
}

fn changed_files(thread: Thread) -> Option<usize> {
    thread.changed_files().filter(|&files| files > 0)
}

fn unreachable(thread: Thread) -> Option<ThreadStatus> {
    let status = thread.status();
    let has_items = thread.with_core(|core| !core.conversation().items().is_empty()).unwrap_or(false);
    (status.problem().is_some() && has_items).then_some(status)
}

const DROP_TINT: f32 = 0.08;
const SELECTED_TINT: f32 = 0.12;
const DOCK_EDGE_SOLID: f32 = 40.0;
const DOCK_EDGE_REACH: f32 = 40.0;
const CORNER_DOT_SIZE_OFF_GRID: f32 = 6.0;
const PILL_HOVER_OPACITY: f32 = 0.85;
