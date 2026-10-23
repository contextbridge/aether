use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadEvent, ThreadStatus};
use crate::ui::{EmptyState, NoticeTone};
use clankerdiff_client::ClientState;
use clankerdiff_gpui::{DiffClientEvent, DiffClientView};
use gpui::{AnyElement, App, Context, Entity, EventEmitter, Subscription, Window, div, prelude::*};
use std::sync::Arc;

/// A thread's Changes tab: the agent's view of the working tree's diff, where the user comments
/// on it, stages or discards it, and sends the review back to the agent.
///
/// It shows whatever the thread last heard from the agent's diff server, so it stays current
/// while another tab is showing.
pub(crate) struct Changes {
    thread: Entity<Thread>,
    /// Kept across review states so comments survive later snapshots, and dropped with the
    /// session, since its comments are on a diff that is gone.
    view: Option<(Entity<DiffClientView>, Subscription)>,
    /// What the tab last showed, so that the thread's other changes, such as each streamed
    /// chunk of a reply, neither re-sync nor re-render it.
    shown: Shown,
    _subscriptions: [Subscription; 3],
}

#[derive(Clone)]
struct Shown {
    review: Option<Arc<ClientState>>,
    status: ThreadStatus,
}

/// Asks the thread's view to show its chat again.
pub(crate) struct Dismissed;

impl Changes {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        let subscriptions = [
            cx.observe(&thread, |this, _, cx| {
                let shown = Shown::of(this.thread.read(cx));
                if !shown.same_as(&this.shown) {
                    this.shown = shown;
                    this.sync(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&thread, |this, _, event: &ThreadEvent, cx| this.on_thread_event(event, cx)),
            cx.observe_global::<DesktopTheme>(|this, cx| {
                let theme = DesktopTheme::global(cx).review().clone();
                if let Some((view, _)) = &this.view {
                    view.update(cx, |view, cx| view.set_theme(theme, cx));
                }
            }),
        ];
        let shown = Shown::of(thread.read(cx));
        let mut changes = Self { thread, view: None, shown, _subscriptions: subscriptions };
        changes.sync(cx);
        changes
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.shown.review.clone() else {
            self.view = None;
            return;
        };
        let (view, _) = self.view.get_or_insert_with(|| {
            let theme = DesktopTheme::global(cx).review().clone();
            let view = cx.new(|_| DiffClientView::new(theme));
            let subscription =
                cx.subscribe(&view, |this, _, event: &DiffClientEvent, cx| this.on_view_event(event, cx));
            (view, subscription)
        });
        view.update(cx, |view, cx| view.set_state(&state, cx));
    }

    fn on_view_event(&mut self, event: &DiffClientEvent, cx: &mut Context<Self>) {
        match event {
            DiffClientEvent::Command(event) => {
                self.thread.update(cx, |thread, _| thread.send_review_event(event.clone()));
            }
            DiffClientEvent::Submit(submission) => {
                self.thread.update(cx, |thread, cx| thread.submit_review(&submission.formatted, cx));
            }
            DiffClientEvent::Cancel => cx.emit(Dismissed),
        }
    }

    fn on_thread_event(&mut self, event: &ThreadEvent, cx: &mut Context<Self>) {
        let Some((view, _)) = &self.view else {
            return;
        };
        view.update(cx, |view, cx| match event {
            ThreadEvent::ReviewActionFinished(result) => view.command_finished(result.clone(), cx),
            ThreadEvent::ReviewSubmitted => view.clear_review(cx),
            _ => {}
        });
    }
}

impl EventEmitter<Dismissed> for Changes {}

impl Render for Changes {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((view, _)) = &self.view {
            return view.clone().into_any_element();
        }
        match self.shown.status {
            ThreadStatus::Connecting | ThreadStatus::Idle | ThreadStatus::Running | ThreadStatus::NeedsInput => {
                render_status("Loading changes…", "Waiting for the agent", cx)
            }
            _ => render_status("Agent not running", "Start the thread again to review its changes", cx),
        }
    }
}

impl Shown {
    fn of(thread: &Thread) -> Self {
        Self { review: thread.review().cloned(), status: thread.status() }
    }

    /// Review states are compared by identity, since the thread replaces its state whenever the
    /// agent reports a new one.
    fn same_as(&self, other: &Self) -> bool {
        let same_review = match (&self.review, &other.review) {
            (Some(review), Some(other)) => Arc::ptr_eq(review, other),
            (review, other) => review.is_none() && other.is_none(),
        };
        same_review && self.status == other.status
    }
}

fn render_status(title: &'static str, detail: &'static str, cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "changes-status".to_owned())
        .size_full()
        .child(EmptyState::new(title, detail, NoticeTone::Neutral, DesktopTheme::global(cx).ui()))
        .into_any_element()
}
