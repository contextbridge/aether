use crate::artifact_review::ArtifactReview;
use crate::chat::{Composer, PickerClosed, SessionOptions, StatusLine, Transcript, reconnect_banner};
use crate::elicitation::{ElicitationCard, OpenReview};
use crate::files::{Dismissed, Files};
use crate::registry::Registry;
use crate::sidebar;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadEvent, ThreadStatus, observe_changes, observe_thread};
use crate::ui::tokens::{GROUP_HEIGHT, ICON_SIZE_SMALL, RADIUS_CONTROL, RADIUS_INNER, ROW_HEIGHT, rem};
use crate::ui::{Icon, IconName, StyledExt, Typography, TypographyExt, UiTheme, WindowDragExt, tagged, text_selector};
use gpui::{
    AnyElement, AnyView, App, AppContext, Context, Div, Entity, ExternalPaths, Hsla, Role, Stateful, StyleRefinement,
    Subscription, Window, div, prelude::*,
};

pub(crate) struct ThreadView {
    thread: Entity<Thread>,
    registry: Entity<Registry>,
    tab: ThreadTab,
    transcript: Entity<Transcript>,
    status: Entity<StatusLine>,
    options: Entity<SessionOptions>,
    changes_seen: Option<usize>,
    elicitation: Entity<ElicitationCard>,
    composer: Entity<Composer>,
    files: Entity<Files>,
    review: Entity<ArtifactReview>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadTab {
    Chat,
    Files,
    Review,
}

impl ThreadView {
    pub(crate) fn new(
        thread: Entity<Thread>,
        registry: Entity<Registry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let transcript = cx.new(|cx| Transcript::new(thread.clone(), cx));
        let status = cx.new(|cx| StatusLine::new(thread.clone(), cx));
        let options = cx.new(|cx| SessionOptions::new(thread.clone(), cx));
        let elicitation = cx.new(|cx| ElicitationCard::new(thread.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(thread.clone(), window, cx));
        let files = cx.new(|cx| Files::new(thread.clone(), cx));
        let review = cx.new(|cx| ArtifactReview::new(thread.clone(), cx));
        let subscriptions = vec![
            observe_thread(
                &thread,
                cx,
                |thread| {
                    (
                        thread.title(),
                        thread.status(),
                        files_label(thread),
                        thread.pending_review().is_some(),
                        unreachable(thread),
                    )
                },
                |this, cx| {
                    this.see_changes(cx);
                    cx.notify();
                },
            ),
            observe_changes(&registry, cx, header(&thread), |_, cx| cx.notify()),
            cx.observe(&files, |_, _, cx| cx.notify()),
            cx.subscribe_in(&options, window, |this, _, _: &PickerClosed, window, cx| this.focus(window, cx)),
            cx.subscribe_in(&elicitation, window, |this, _, _: &OpenReview, window, cx| {
                this.show(ThreadTab::Review, window, cx);
            }),
            cx.subscribe_in(&thread, window, |this, _, event: &ThreadEvent, window, cx| {
                if *event == ThreadEvent::ReviewSubmitted {
                    this.show(ThreadTab::Chat, window, cx);
                }
            }),
            cx.subscribe_in(&files, window, |this, _, _: &Dismissed, window, cx| {
                this.show(ThreadTab::Chat, window, cx);
            }),
            cx.subscribe_in(&review, window, |this, _, _: &Dismissed, window, cx| {
                if this.tab == ThreadTab::Review {
                    this.show(ThreadTab::Chat, window, cx);
                }
            }),
        ];
        Self {
            thread,
            registry,
            tab: ThreadTab::Chat,
            transcript,
            status,
            options,
            changes_seen: None,
            elicitation,
            composer,
            files,
            review,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn show(&mut self, tab: ThreadTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == ThreadTab::Review && self.thread.read(cx).pending_review().is_none() {
            return;
        }
        self.tab = tab;
        self.see_changes(cx);
        self.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        match self.tab {
            ThreadTab::Chat => self.composer.update(cx, |composer, cx| composer.focus(window, cx)),
            ThreadTab::Files | ThreadTab::Review => window.blur(cx),
        }
    }

    fn see_changes(&mut self, cx: &App) {
        if self.tab == ThreadTab::Files {
            self.changes_seen = changed_files(self.thread.read(cx));
        }
    }

    fn render_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = DesktopTheme::global(cx).ui();
        let colors = theme.colors;
        let thread = self.thread.read(cx);
        let files = changed_files(thread);
        let reviewing = thread.pending_review().is_some();
        let files_label = files_label(thread);
        let agent = match thread.status() {
            _ if self.tab == ThreadTab::Chat => None,
            ThreadStatus::NeedsInput => Some(("chat-waiting", colors.warning)),
            ThreadStatus::Running | ThreadStatus::Background => Some(("chat-working", colors.info)),
            _ => None,
        };
        let unseen = files.is_some_and(|files| self.tab != ThreadTab::Files && self.changes_seen != Some(files));
        let tab = |id: &'static str, icon: IconName, label: &'static str, value: ThreadTab, cx: &mut Context<Self>| {
            segment(id, icon, label, self.tab == value, &theme)
                .on_click(cx.listener(move |this, _, window, cx| this.show(value, window, cx)))
        };
        let chat = tab("tab-chat", IconName::MessageSquareText, "Chat", ThreadTab::Chat, cx)
            .children(agent.map(|(selector, color)| corner_dot(selector, color)));
        let files_tab = tab("tab-files", IconName::Folder, "Files", ThreadTab::Files, cx)
            .aria_label(files_label)
            .children(files.map(|files| {
                let count = files.to_string();
                div()
                    .debug_selector(|| text_selector("changes-count", &count))
                    .text_color(colors.text_muted)
                    .child(count)
            }))
            .when(unseen, |segment| segment.child(corner_dot("changes-unseen", colors.accent)));
        let review = reviewing.then(|| tab("tab-review", IconName::Eye, "Review", ThreadTab::Review, cx));
        div()
            .id("view-switch")
            .debug_selector(|| "view-switch".to_owned())
            .role(Role::TabList)
            .no_window_drag()
            .flex_none()
            .h(rem(ROW_HEIGHT))
            .p_1()
            .flex()
            .items_center()
            .gap_1()
            .rounded(rem(RADIUS_CONTROL))
            .border_1()
            .border_color(colors.border_subtle)
            .bg(colors.input)
            .typography(Typography::Small)
            .child(chat)
            .child(files_tab)
            .children(review)
            .into_any_element()
    }

    fn render_review_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let comments = self.files.read(cx).comments();
        if self.tab != ThreadTab::Files || comments == 0 {
            return None;
        }
        let theme = DesktopTheme::global(cx).ui();
        let colors = theme.colors;
        let copy = pill_button("review-copy", "Copy", false, &theme).on_click(cx.listener(|this, _, window, cx| {
            this.files.update(cx, |files, cx| files.copy(window, cx));
        }));
        let submit =
            pill_button("review-submit", "Submit review", true, &theme).on_click(cx.listener(|this, _, window, cx| {
                this.files.update(cx, |files, cx| files.submit(window, cx));
            }));
        let label = comments_label(comments);
        let count = div()
            .debug_selector(|| text_selector("review-comments", &label))
            .mr_2()
            .whitespace_nowrap()
            .text_color(colors.canvas.opacity(PILL_MUTED_OPACITY))
            .child(label);
        let pill = div()
            .id("review-pill")
            .debug_selector(|| "review-pill".to_owned())
            .occlude()
            .h(rem(GROUP_HEIGHT))
            .pl_4()
            .pr_1()
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .bg(colors.text)
            .text_color(colors.canvas)
            .shadow_lg()
            .typography(Typography::Small)
            .child(count)
            .child(copy)
            .child(submit);
        Some(div().absolute().left_0().right_0().bottom_4().flex().justify_center().child(pill).into_any_element())
    }
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let state = self.registry.read(cx).state();
        let thread = self.thread.read(cx);
        let (title, sidebar_collapsed) = (state.title(thread.session_id(), thread.title()), state.sidebar_collapsed);
        let project = thread.project().map(|project| project.display().to_string());
        let agent = thread.target().label().to_owned();
        let unreachable = unreachable(thread);
        let header = div()
            .debug_selector(|| "thread-header".to_owned())
            .top_bar(sidebar_collapsed, window)
            .pr_4()
            .gap_3()
            .bg(theme.colors.chrome)
            .when(sidebar_collapsed, |header| header.child(sidebar::toggle_button(false, &theme)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| text_selector("thread-title", &title))
                            .min_w_0()
                            .truncate()
                            .typography(Typography::Title)
                            .child(title),
                    )
                    .when_some(project, |row, project| {
                        row.child(div().min_w_0().truncate().muted(&theme).child(project))
                    })
                    .child(div().debug_selector(|| "thread-agent".to_owned()).flex_none().muted(&theme).child(agent)),
            )
            .child(self.render_switch(cx));
        let view = match self.tab {
            ThreadTab::Chat => AnyView::from(self.transcript.clone()),
            ThreadTab::Files => AnyView::from(self.files.clone()),
            ThreadTab::Review => AnyView::from(self.review.clone()),
        }
        .cached(StyleRefinement::default().size_full());
        let chat = self.tab == ThreadTab::Chat;
        let dock = chat.then(|| {
            let bar = div()
                .debug_selector(|| "composer-bar".to_owned())
                .h(rem(ROW_HEIGHT))
                .mt_1()
                .px_2()
                .flex()
                .items_center()
                .gap_4()
                .child(self.options.clone())
                .child(self.status.clone());
            div().flex_none().w_full().px_4().pb_3().child(
                div()
                    .content_width()
                    .flex()
                    .flex_col()
                    .children(
                        unreachable.and_then(|status| reconnect_banner(&self.thread, status, &theme)).map(Styled::mt_2),
                    )
                    .child(self.elicitation.clone())
                    .child(self.composer.clone())
                    .child(bar),
            )
        });
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when(chat, |body| {
                body.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(theme.colors.accent.opacity(DROP_TINT)))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                        this.composer.update(cx, |composer, cx| {
                            composer.attach(paths.paths(), window, cx);
                            composer.focus(window, cx);
                        });
                    }))
            })
            .child(div().relative().flex_1().min_h_0().w_full().child(view).children(self.render_review_pill(cx)))
            .children(dock);
        div().size_full().flex().flex_col().child(header).child(body)
    }
}

const DROP_TINT: f32 = 0.08;

const CORNER_DOT_SIZE_OFF_GRID: f32 = 6.0;

fn segment(id: &'static str, icon: IconName, label: &'static str, selected: bool, theme: &UiTheme) -> Stateful<Div> {
    let colors = theme.colors;
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .role(Role::Tab)
        .aria_selected(selected)
        .aria_label(label)
        .focusable()
        .tab_stop(true)
        .relative()
        .h_full()
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded(rem(RADIUS_INNER))
        .cursor_pointer()
        .text_color(if selected { colors.text } else { colors.text_secondary })
        .when(selected, |segment| segment.bg(colors.elevated).shadow_sm())
        .hover(move |segment| segment.text_color(colors.text))
        .focus_visible(move |segment| segment.text_color(colors.text))
        .child(tagged(format!("{id}-icon"), Icon::new(icon, colors.text_muted).size(ICON_SIZE_SMALL)))
        .child(label)
}

fn corner_dot(selector: &'static str, color: Hsla) -> Div {
    div()
        .debug_selector(move || selector.to_owned())
        .absolute()
        .top_0()
        .right_0()
        .size(rem(CORNER_DOT_SIZE_OFF_GRID))
        .rounded_full()
        .bg(color)
}

fn pill_button(id: &'static str, label: &'static str, primary: bool, theme: &UiTheme) -> Stateful<Div> {
    let colors = theme.colors;
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .role(Role::Button)
        .aria_label(label)
        .h_full()
        .px_3()
        .flex()
        .items_center()
        .rounded_full()
        .whitespace_nowrap()
        .cursor_pointer()
        .when(primary, |button| button.bg(colors.canvas).text_color(colors.text))
        .hover(move |button| {
            if primary { button.opacity(PILL_HOVER_OPACITY) } else { button.bg(colors.canvas.opacity(PILL_HOVER_TINT)) }
        })
        .child(label)
}

const PILL_MUTED_OPACITY: f32 = 0.65;
const PILL_HOVER_OPACITY: f32 = 0.85;
const PILL_HOVER_TINT: f32 = 0.14;

fn comments_label(comments: usize) -> String {
    if comments == 1 { "1 comment".to_owned() } else { format!("{comments} comments") }
}

fn header(thread: &Entity<Thread>) -> impl Fn(&Registry, &App) -> (bool, Option<String>) + 'static {
    let thread = thread.clone();
    move |registry, cx| {
        let state = registry.state();
        let renamed = thread.read(cx).session_id().and_then(|session_id| state.titles.get(session_id));
        (state.sidebar_collapsed, renamed.cloned())
    }
}

fn changed_files(thread: &Thread) -> Option<usize> {
    thread.changed_files().filter(|&files| files > 0)
}

fn files_label(thread: &Thread) -> String {
    match changed_files(thread) {
        Some(files) => format!("Files ({files} changed)"),
        None => "Files".to_owned(),
    }
}

fn unreachable(thread: &Thread) -> Option<ThreadStatus> {
    let status = thread.status();
    let has_items = thread.core().is_some_and(|core| !core.conversation().items().is_empty());
    (status.problem().is_some() && has_items).then_some(status)
}
