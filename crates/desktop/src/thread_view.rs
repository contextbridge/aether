use crate::artifact_review::{ArtifactReview, pending_review};
use crate::changes::{Changes, Dismissed};
use crate::chat::{Composer, PlanPanel, Transcript};
use crate::elicitation::{ElicitationCard, OpenReview};
use crate::registry::Registry;
use crate::sidebar;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadEvent};
use crate::ui::tokens::{BAR_HEIGHT, rem};
use crate::ui::{Button, ButtonVariant, IconName, Typography, TypographyExt};
use gpui::{App, AppContext, Context, Entity, StyleRefinement, Subscription, Window, div, prelude::*};

/// One thread's main area: a header naming the thread and its project, above either its chat
/// (with any question its agent waits on), the changes it made to the working tree, or the
/// artifact it asked the user to review.
///
/// Each thread keeps its own view, so switching threads keeps every transcript's scroll position,
/// every composer's draft, every tab choice and every unsent review comment.
pub(crate) struct ThreadView {
    thread: Entity<Thread>,
    registry: Entity<Registry>,
    tab: ThreadTab,
    transcript: Entity<Transcript>,
    plan: Entity<PlanPanel>,
    elicitation: Entity<ElicitationCard>,
    composer: Entity<Composer>,
    changes: Entity<Changes>,
    review: Entity<ArtifactReview>,
    /// The header as last rendered, so that the thread's other changes, such as each streamed
    /// chunk of a reply, do not re-render the view.
    header: Header,
    _subscriptions: Vec<Subscription>,
}

/// What the header shows of the thread.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Header {
    title: String,
    changes_label: String,
    /// Whether the agent waits on an artifact review, which the Review tab shows.
    reviewing: bool,
    sidebar_collapsed: bool,
}

/// What a thread's main area shows. The Review tab exists only while the agent waits on a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadTab {
    Chat,
    Changes,
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
        let plan = cx.new(|cx| PlanPanel::new(thread.clone(), cx));
        let elicitation = cx.new(|cx| ElicitationCard::new(thread.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(thread.clone(), window, cx));
        let changes = cx.new(|cx| Changes::new(thread.clone(), cx));
        let review = cx.new(|cx| ArtifactReview::new(thread.clone(), cx));
        let subscriptions = vec![
            cx.observe_in(&thread, window, |this, _, window, cx| {
                this.sync_header(cx);
                if this.tab == ThreadTab::Review && !this.header.reviewing {
                    this.show(ThreadTab::Chat, window, cx);
                }
            }),
            cx.observe(&registry, |this, _, cx| this.sync_header(cx)),
            cx.subscribe_in(&elicitation, window, |this, _, _: &OpenReview, window, cx| {
                this.show(ThreadTab::Review, window, cx);
            }),
            cx.subscribe_in(&thread, window, |this, _, event: &ThreadEvent, window, cx| {
                if *event == ThreadEvent::ReviewSubmitted {
                    this.show(ThreadTab::Chat, window, cx);
                }
            }),
            cx.subscribe_in(&changes, window, |this, _, _: &Dismissed, window, cx| {
                this.show(ThreadTab::Chat, window, cx);
            }),
        ];
        let header = Header::of(&thread, &registry, cx);
        Self {
            thread,
            registry,
            header,
            tab: ThreadTab::Chat,
            transcript,
            plan,
            elicitation,
            composer,
            changes,
            review,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    pub(crate) fn title(&self, cx: &App) -> String {
        Header::title(&self.thread, &self.registry, cx)
    }

    /// The Changes tab's label, counting the files the agent's changes touch once it reports them.
    pub(crate) fn changes_label(&self) -> String {
        self.header.changes_label.clone()
    }

    /// Shows `tab` and moves focus into it. The Review tab is not shown while there is nothing to
    /// review.
    pub(crate) fn show(&mut self, tab: ThreadTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == ThreadTab::Review && !self.header.reviewing {
            return;
        }
        self.tab = tab;
        self.focus(window, cx);
        cx.notify();
    }

    /// Focuses the shown tab: the composer of the chat, or the viewer of the changes or review.
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        match self.tab {
            ThreadTab::Chat => self.composer.update(cx, |composer, cx| composer.focus(window, cx)),
            // The diff and Markdown viewers keep their focus handles to themselves and take focus
            // when they paint into a window where nothing has it.
            ThreadTab::Changes | ThreadTab::Review => window.blur(cx),
        }
    }

    fn sync_header(&mut self, cx: &mut Context<Self>) {
        let header = Header::of(&self.thread, &self.registry, cx);
        if header != self.header {
            self.header = header;
            cx.notify();
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let tab = |id: &'static str, icon: IconName, label: String, value: ThreadTab, cx: &mut Context<Self>| {
            Button::new(id, label, theme)
                .icon(icon)
                .variant(ButtonVariant::Ghost)
                .selected(self.tab == value)
                .on_click(cx.listener(move |this, _, window, cx| this.show(value, window, cx)))
        };
        let review =
            self.header.reviewing.then(|| tab("tab-review", IconName::Eye, "Review".to_owned(), ThreadTab::Review, cx));
        div()
            .flex_none()
            .flex()
            .gap_1()
            .child(tab("tab-chat", IconName::MessageSquareText, "Chat".to_owned(), ThreadTab::Chat, cx))
            .child(tab("tab-changes", IconName::GitCompare, self.header.changes_label.clone(), ThreadTab::Changes, cx))
            .children(review)
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let title = self.header.title.clone();
        let thread = self.thread.read(cx);
        let project = thread.project().display().to_string();
        let agent = thread.target().agent.name.clone();
        let header = div()
            .debug_selector(|| "thread-header".to_owned())
            .flex_none()
            .h(rem(BAR_HEIGHT))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.colors.border)
            .when(self.header.sidebar_collapsed, |header| header.child(sidebar::toggle_button(false, &theme)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().min_w_0().truncate().typography(Typography::Title).child(title))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .typography(Typography::Small)
                            .text_color(theme.colors.text_muted)
                            .child(project),
                    )
                    .child(
                        div()
                            .debug_selector(|| "thread-agent".to_owned())
                            .flex_none()
                            .typography(Typography::Small)
                            .text_color(theme.colors.text_muted)
                            .child(agent),
                    ),
            )
            .child(self.render_tabs(cx));
        let body = match self.tab {
            ThreadTab::Chat => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.transcript.clone().cached(StyleRefinement::default().flex_1().min_h_0().w_full()))
                .child(self.plan.clone())
                .child(self.elicitation.clone())
                .child(self.composer.clone()),
            ThreadTab::Changes => {
                div().flex_1().min_h_0().child(self.changes.clone().cached(StyleRefinement::default().size_full()))
            }
            ThreadTab::Review => {
                div().flex_1().min_h_0().child(self.review.clone().cached(StyleRefinement::default().size_full()))
            }
        };
        div().size_full().flex().flex_col().child(header).child(body)
    }
}

impl Header {
    fn of(thread: &Entity<Thread>, registry: &Entity<Registry>, cx: &App) -> Self {
        let changes_label = match thread.read(cx).changed_files() {
            Some(files) => format!("Changes ({files})"),
            None => "Changes".to_owned(),
        };
        let reviewing = pending_review(thread.read(cx)).is_some();
        let sidebar_collapsed = registry.read(cx).state().sidebar_collapsed;
        Self { title: Self::title(thread, registry, cx), changes_label, reviewing, sidebar_collapsed }
    }

    fn title(thread: &Entity<Thread>, registry: &Entity<Registry>, cx: &App) -> String {
        let thread = thread.read(cx);
        registry.read(cx).state().title(thread.session_id(), thread.title())
    }
}
