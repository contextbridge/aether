use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadEvent, observe_thread};
use crate::ui::{EmptyState, NoticeTone};
use clankerdiff_core::{DiffReviewCommand, DiffReviewEvent};
use clankerdiff_gpui::{DiffClientEvent, DiffClientView, DiffViewerOptions};
use gpui::{App, Context, Entity, EventEmitter, Subscription, Window, div, prelude::*};
use std::sync::Arc;

pub(crate) struct Files {
    thread: Entity<Thread>,
    view: Option<(Entity<DiffClientView>, Subscription)>,
    comments: usize,
    _subscriptions: [Subscription; 3],
}

pub(crate) struct Dismissed;

impl Files {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        let subscriptions = [
            observe_thread(
                &thread,
                cx,
                |thread| (thread.review().map(Arc::as_ptr), thread.status()),
                |this, cx| {
                    this.sync(cx);
                    cx.notify();
                },
            ),
            cx.subscribe(&thread, |this, _, event: &ThreadEvent, cx| this.on_thread_event(event, cx)),
            cx.observe_global::<DesktopTheme>(|this, cx| {
                let theme = DesktopTheme::global(cx).review().clone();
                if let Some((view, _)) = &this.view {
                    view.update(cx, |view, cx| view.set_theme(theme, cx));
                }
            }),
        ];
        let mut files = Self { thread, view: None, comments: 0, _subscriptions: subscriptions };
        files.sync(cx);
        files
    }

    pub(crate) fn comments(&self) -> usize {
        self.comments
    }

    pub(crate) fn submit(&self, window: &mut Window, cx: &mut App) {
        self.run(DiffReviewCommand::SubmitReview, window, cx);
    }

    pub(crate) fn copy(&self, window: &mut Window, cx: &mut App) {
        self.run(DiffReviewCommand::CopyReview, window, cx);
    }

    fn run(&self, command: DiffReviewCommand, window: &mut Window, cx: &mut App) {
        if let Some((view, _)) = &self.view {
            view.update(cx, |view, cx| view.handle_command(command, window, cx));
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.thread.read(cx).review().cloned() else {
            self.view = None;
            self.comments = 0;
            return;
        };
        let (view, _) = self.view.get_or_insert_with(|| {
            let theme = DesktopTheme::global(cx).review().clone();
            let options = DiffViewerOptions { themes: false, review_bar: false, ..DiffViewerOptions::default() };
            let view = cx.new(|_| DiffClientView::with_options(theme, options));
            let subscription =
                cx.subscribe(&view, |this, _, event: &DiffClientEvent, cx| this.on_view_event(event, cx));
            (view, subscription)
        });
        view.update(cx, |view, cx| view.set_state(&state, cx));
    }

    fn on_view_event(&mut self, event: &DiffClientEvent, cx: &mut Context<Self>) {
        let review_event = match event {
            DiffClientEvent::Command(event) => event.clone(),
            DiffClientEvent::Lookup(lookup) => DiffReviewEvent::Lookup(lookup.clone()),
            DiffClientEvent::Submit(submission) => {
                return Thread::with_core(&self.thread, cx, |core| core.submit_review(&submission.formatted));
            }
            DiffClientEvent::ReviewChanged { comments } => {
                self.comments = *comments;
                return cx.notify();
            }
            DiffClientEvent::Cancel => return cx.emit(Dismissed),
        };
        Thread::with_core(&self.thread, cx, |core| core.send_review_event(review_event));
    }

    fn on_thread_event(&mut self, event: &ThreadEvent, cx: &mut Context<Self>) {
        let Some((view, _)) = &self.view else {
            return;
        };
        view.update(cx, |view, cx| match event {
            ThreadEvent::ReviewActionFinished(result) => {
                view.command_finished(result.clone().map_err(|error| error.to_string()), cx);
            }
            ThreadEvent::ReviewLookupFinished(reply) => view.lookup_finished(reply.clone(), cx),
            ThreadEvent::ReviewSubmitted => view.clear_review(cx),
            _ => {}
        });
    }
}

impl EventEmitter<Dismissed> for Files {}

impl Render for Files {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((view, _)) = &self.view {
            return view.clone().into_any_element();
        }
        let thread = self.thread.read(cx);
        let title = if thread.status().is_stopped() { "Agent not running" } else { "Waiting for the agent" };
        let detail = thread.listed_files().err().unwrap_or_default();
        div()
            .debug_selector(|| "files-status".to_owned())
            .size_full()
            .child(EmptyState::new(title, detail, NoticeTone::Neutral, DesktopTheme::global(cx).ui()))
            .into_any_element()
    }
}
