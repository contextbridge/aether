use crate::theme::DesktopTheme;
use crate::threads::Thread;
use clankerdiff_core::ReviewCapabilities;
use clankerdiff_gpui::{MarkdownReviewer, MarkdownReviewerOptions};
use clankerdiff_markdown::{MarkdownDocument, MarkdownReviewDecision, MarkdownReviewEvent, MarkdownReviewSubmission};
use client_core::elicitation::ElicitationResponder;
use client_core::session_core::Elicitation;
use gpui::{ClipboardItem, Context, Entity, Subscription, Window, div, prelude::*};
use std::sync::Arc;
use utils::artifact_review::{ArtifactReviewElicitationMeta, ArtifactReviewSubmission};

/// A thread's Review tab: the Markdown artifact its agent asked the user to review, where the
/// user comments on it and approves it or sends the comments back.
///
/// It shows a reviewer only while the agent waits on the review, so answering it, from here or
/// by declining its card, takes the reviewer away.
pub(crate) struct ArtifactReview {
    thread: Entity<Thread>,
    reviewer: Option<Reviewer>,
    _subscriptions: Vec<Subscription>,
}

/// The artifact review `thread`'s agent waits on, if any.
pub(crate) fn pending_review(thread: &Thread) -> Option<&ArtifactReviewElicitationMeta> {
    match thread.elicitation() {
        Some(Elicitation::ArtifactReview(meta)) => Some(meta),
        _ => None,
    }
}

struct Reviewer {
    view: Entity<MarkdownReviewer>,
    _subscription: Subscription,
}

impl ArtifactReview {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&thread, |this, _, cx| this.sync(cx)),
            cx.observe_global::<DesktopTheme>(|this, cx| {
                let theme = DesktopTheme::global(cx).review().clone();
                if let Some(reviewer) = &this.reviewer {
                    reviewer.view.update(cx, |view, cx| view.set_theme(theme, cx));
                }
            }),
        ];
        let mut review = Self { thread, reviewer: None, _subscriptions: subscriptions };
        review.sync(cx);
        review
    }

    /// Starts reviewing an artifact when one arrives, and drops it once it is answered or
    /// withdrawn.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let meta = pending_review(self.thread.read(cx)).cloned();
        match (meta, &self.reviewer) {
            (Some(_), Some(_)) | (None, None) => {}
            (Some(meta), None) => {
                self.reviewer = Some(Reviewer::new(meta, cx));
                cx.notify();
            }
            (None, Some(_)) => {
                self.reviewer = None;
                cx.notify();
            }
        }
    }

    fn on_reviewer_event(&mut self, event: &MarkdownReviewEvent, cx: &mut Context<Self>) {
        match event {
            MarkdownReviewEvent::Submit(submission) => {
                let answer = answer(submission);
                self.thread
                    .update(cx, |thread, cx| thread.answer(|responder| responder.accept_strings(answer.fields()), cx));
            }
            MarkdownReviewEvent::CopyFormatted(text) => cx.write_to_clipboard(ClipboardItem::new_string(text.clone())),
            MarkdownReviewEvent::Cancel => {
                self.thread.update(cx, |thread, cx| thread.answer(ElicitationResponder::cancel, cx));
            }
        }
    }
}

impl Render for ArtifactReview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "artifact-review".to_owned())
            .size_full()
            .children(self.reviewer.as_ref().map(|reviewer| reviewer.view.clone()))
    }
}

impl Reviewer {
    fn new(meta: ArtifactReviewElicitationMeta, cx: &mut Context<ArtifactReview>) -> Self {
        let ArtifactReviewElicitationMeta { path, title, markdown, .. } = meta;
        let document =
            MarkdownDocument::parse_with_metadata(path.map(|path| path.display().to_string()), Some(title), markdown);
        let theme = DesktopTheme::global(cx).review().clone();
        let view = cx.new(|cx| {
            let mut reviewer =
                MarkdownReviewer::with_options(Arc::new(document), theme, MarkdownReviewerOptions::default());
            reviewer.set_capabilities(
                ReviewCapabilities { repository: false, refresh: false, scope: false, submit: true, clipboard: true },
                cx,
            );
            reviewer
        });
        let subscription =
            cx.subscribe(&view, |this, _, event: &MarkdownReviewEvent, cx| this.on_reviewer_event(event, cx));
        Self { view, _subscription: subscription }
    }
}

/// The agent hears an approval only when nothing was commented, since an approval carries no
/// feedback.
fn answer(submission: &MarkdownReviewSubmission) -> ArtifactReviewSubmission {
    if submission.decision == MarkdownReviewDecision::Approved && submission.comments.is_empty() {
        ArtifactReviewSubmission::Approved
    } else {
        ArtifactReviewSubmission::Feedback { feedback: submission.formatted.clone() }
    }
}
