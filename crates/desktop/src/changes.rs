use crate::theme::DesktopTheme;
use crate::threads::{ReviewEvent, Thread, ThreadStatus};
use clankerdiff_client::{ClientState, ConnectionState, DiffReviewEvent, DiffScope, DiffSnapshot};
use clankerdiff_gpui::ui::prelude::{Button, ControlSize, EmptyState, NoticeTone};
use clankerdiff_gpui::{DiffViewer, DiffViewerOptions};
use gpui::{AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, Subscription, Window, div, prelude::*};
use std::sync::Arc;

/// A thread's Changes tab: the agent's view of the working tree's diff, where the user comments
/// on it, stages or discards it, and sends the review back to the agent.
///
/// It shows whatever the thread last heard from the agent's diff server, so it stays current
/// while another tab is showing.
pub(crate) struct Changes {
    thread: Entity<Thread>,
    /// Created with the first snapshot, and kept so comments survive later snapshots.
    viewer: Option<Entity<DiffViewer>>,
    installed: Option<Arc<DiffSnapshot>>,
    subscriptions: Vec<Subscription>,
}

/// Asks the thread's view to show its chat again.
pub(crate) struct Dismissed;

impl Changes {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&thread, |this, _, cx| this.sync(cx)),
            cx.subscribe(&thread, |this, _, event: &ReviewEvent, cx| this.on_review_event(event, cx)),
            cx.observe_global::<DesktopTheme>(|this, cx| {
                let theme = DesktopTheme::global(cx).review().clone();
                if let Some(viewer) = &this.viewer {
                    viewer.update(cx, |viewer, cx| viewer.set_theme(theme, cx));
                }
            }),
        ];
        let mut changes = Self { thread, viewer: None, installed: None, subscriptions };
        changes.sync(cx);
        changes
    }

    /// Shows the thread's latest review state. Once the session closes there is none, and the
    /// comments on the old diff go with it.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.thread.read(cx).review().cloned() else {
            self.viewer = None;
            self.installed = None;
            return;
        };
        if let Some(snapshot) = state.snapshot_if_changed(&mut self.installed) {
            self.install(snapshot, cx);
        }
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| {
                viewer.set_capabilities(state.capabilities, cx);
                viewer.set_background_error(state.status(), cx);
            });
        }
    }

    fn install(&mut self, snapshot: &DiffSnapshot, cx: &mut Context<Self>) {
        let document = snapshot.document.clone();
        let scope = snapshot.scope;
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| {
                viewer.set_scope(scope, cx);
                viewer.set_document(document, cx);
            });
            return;
        }
        let theme = DesktopTheme::global(cx).review().clone();
        let viewer = cx.new(|cx| {
            let mut viewer = DiffViewer::with_options(document, theme, DiffViewerOptions::default());
            viewer.set_scope(scope, cx);
            viewer
        });
        self.subscriptions.push(cx.subscribe(&viewer, |this, _, event: &DiffReviewEvent, cx| {
            this.on_viewer_event(event.clone(), cx);
        }));
        self.viewer = Some(viewer);
    }

    fn on_viewer_event(&mut self, event: DiffReviewEvent, cx: &mut Context<Self>) {
        match event {
            DiffReviewEvent::SubmitReview(submission) => {
                self.thread.update(cx, |thread, cx| thread.submit_review(&submission.formatted, cx));
            }
            DiffReviewEvent::CopyFormattedReview(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            DiffReviewEvent::Cancel => cx.emit(Dismissed),
            event
            @ (DiffReviewEvent::RepositoryAction(_) | DiffReviewEvent::SetScope(_) | DiffReviewEvent::Refresh) => {
                self.send(event, cx);
            }
        }
    }

    /// Hands `event` to the agent, holding further repository actions until it finishes.
    fn send(&mut self, event: DiffReviewEvent, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| viewer.set_repository_pending(true, cx));
        }
        self.thread.update(cx, |thread, _| thread.send_review_event(event));
    }

    fn on_review_event(&mut self, event: &ReviewEvent, cx: &mut Context<Self>) {
        let Some(viewer) = &self.viewer else {
            return;
        };
        viewer.update(cx, |viewer, cx| match event {
            ReviewEvent::ActionFinished(Ok(())) => viewer.set_repository_pending(false, cx),
            ReviewEvent::ActionFinished(Err(error)) => viewer.set_repository_error(error.clone(), cx),
            ReviewEvent::Submitted => viewer.clear_review(cx),
        });
    }

    /// A clean working tree, with a way to look at another scope, since the viewer that would
    /// otherwise offer it has nothing to show.
    fn render_clean(scope: DiffScope, cx: &mut Context<Self>) -> AnyElement {
        let theme = DesktopTheme::global(cx).ui();
        let segment = |id: &'static str, label: &'static str, value: DiffScope| {
            Button::new(id, label, theme).size(ControlSize::Small).selected(scope == value).on_click(cx.listener(
                move |this, _, _, cx| {
                    if scope != value {
                        this.send(DiffReviewEvent::SetScope(value), cx);
                    }
                },
            ))
        };
        div()
            .debug_selector(|| "changes-clean".to_owned())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(EmptyState::new("No changes", format!("Nothing to review in {scope}"), NoticeTone::Neutral, theme))
            .child(div().flex().gap_2().children([
                segment("scope-unstaged", "Unstaged", DiffScope::Unstaged),
                segment("scope-staged", "Staged", DiffScope::Staged),
                segment("scope-both", "Both", DiffScope::Both),
            ]))
            .into_any_element()
    }
}

impl EventEmitter<Dismissed> for Changes {}

impl Render for Changes {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = self.thread.read(cx);
        let Some(state) = thread.review().cloned() else {
            return match thread.status() {
                ThreadStatus::Connecting | ThreadStatus::Idle | ThreadStatus::Running | ThreadStatus::NeedsInput => {
                    render_status("Loading changes…", "Waiting for the agent".to_owned(), NoticeTone::Info, cx)
                }
                _ => render_status(
                    "Agent not running",
                    "Start the thread again to review its changes".to_owned(),
                    NoticeTone::Neutral,
                    cx,
                ),
            };
        };
        match (&state.snapshot, &self.viewer) {
            (Some(snapshot), _) if snapshot.document.files.is_empty() => Self::render_clean(snapshot.scope, cx),
            (Some(_), Some(viewer)) => div()
                .debug_selector(|| "changes-viewer".to_owned())
                .size_full()
                .child(viewer.clone())
                .into_any_element(),
            _ => render_unavailable(&state, cx),
        }
    }
}

/// Before the first snapshot: still loading, or the agent could not read the repository.
fn render_unavailable(state: &ClientState, cx: &App) -> AnyElement {
    let detail = state.status().unwrap_or_default();
    match state.connection {
        ConnectionState::Failed(_) => render_status("Could not load changes", detail, NoticeTone::Error, cx),
        ConnectionState::Connecting | ConnectionState::Connected => {
            render_status("Loading changes…", detail, NoticeTone::Info, cx)
        }
    }
}

fn render_status(title: &str, detail: String, tone: NoticeTone, cx: &App) -> AnyElement {
    let theme = DesktopTheme::global(cx).ui();
    div()
        .debug_selector(|| "changes-status".to_owned())
        .size_full()
        .child(EmptyState::new(title.to_owned(), detail, tone, theme))
        .into_any_element()
}
