use crate::changes::{Changes, Dismissed};
use crate::chat::{Composer, PlanPanel, Transcript};
use crate::elicitation::ElicitationCard;
use crate::registry::Registry;
use crate::theme::DesktopTheme;
use crate::threads::{ReviewEvent, Thread};
use crate::ui::tokens::{BAR_HEIGHT, rem};
use crate::ui::{Button, ButtonVariant, Typography, TypographyExt};
use gpui::{App, AppContext, Context, Entity, StyleRefinement, Subscription, Window, div, prelude::*};

/// One thread's main area: a header naming the thread and its project, above either its chat
/// (with any question its agent waits on) or the changes it made to the working tree.
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
}

/// The two halves of a thread's main area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadTab {
    Chat,
    Changes,
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
        let subscriptions = vec![
            cx.observe(&thread, |this, _, cx| this.sync_header(cx)),
            cx.observe(&registry, |this, _, cx| this.sync_header(cx)),
            cx.subscribe_in(&thread, window, |this, _, event: &ReviewEvent, window, cx| {
                if *event == ReviewEvent::Submitted {
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
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// The title the user gave the thread, or else the one derived from its first prompt.
    pub(crate) fn title(&self, cx: &App) -> String {
        Header::title(&self.thread, &self.registry, cx)
    }

    /// The Changes tab's label, counting the files the agent's changes touch once it reports them.
    pub(crate) fn changes_label(&self) -> String {
        self.header.changes_label.clone()
    }

    /// Shows `tab` and moves focus into it.
    pub(crate) fn show(&mut self, tab: ThreadTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.focus(window, cx);
        cx.notify();
    }

    /// Focuses the shown tab: the composer of the chat, or the diff viewer of the changes.
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        match self.tab {
            ThreadTab::Chat => self.composer.update(cx, |composer, cx| composer.focus(window, cx)),
            // The diff viewer keeps its focus handle to itself and takes focus when it paints
            // into a window where nothing has it.
            ThreadTab::Changes => window.blur(cx),
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
        let tab = |id: &'static str, label: String, value: ThreadTab, cx: &mut Context<Self>| {
            Button::new(id, label, theme)
                .variant(ButtonVariant::Ghost)
                .selected(self.tab == value)
                .on_click(cx.listener(move |this, _, window, cx| this.show(value, window, cx)))
        };
        div().flex_none().flex().gap_1().child(tab("tab-chat", "Chat".to_owned(), ThreadTab::Chat, cx)).child(tab(
            "tab-changes",
            self.header.changes_label.clone(),
            ThreadTab::Changes,
            cx,
        ))
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let title = self.header.title.clone();
        let project = self.thread.read(cx).project().display().to_string();
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
        Self { title: Self::title(thread, registry, cx), changes_label }
    }

    fn title(thread: &Entity<Thread>, registry: &Entity<Registry>, cx: &App) -> String {
        let thread = thread.read(cx);
        let title = thread.title();
        match thread.session_id() {
            Some(session_id) => registry.read(cx).thread(session_id).title_or(title),
            None => title,
        }
    }
}
