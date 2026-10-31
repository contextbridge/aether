use crate::files::Dismissed;
use crate::theme::DesktopTheme;
use crate::threads::Thread;
use clankerdiff_core::ReviewCapabilities;
use clankerdiff_gpui::{MarkdownReviewer, MarkdownReviewerOptions};
use clankerdiff_markdown::{MarkdownDocument, MarkdownReviewDecision, MarkdownReviewEvent, MarkdownReviewSubmission};
use client_core::elicitation::{ElicitationAnswer, ElicitationId};
use gpui::{ClipboardItem, Context, Entity, EventEmitter, Subscription, Window, div, prelude::*};
use std::sync::Arc;
use utils::artifact_review::{ArtifactReviewElicitationMeta, ArtifactReviewSubmission};

pub(crate) struct ArtifactReview {
    thread: Entity<Thread>,
    reviewer: Option<Reviewer>,
    _subscriptions: Vec<Subscription>,
}

struct Reviewer {
    id: ElicitationId,
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

    fn sync(&mut self, cx: &mut Context<Self>) {
        let pending = self.thread.read(cx).pending_review();
        if pending.map(|(id, _)| id) == self.reviewer.as_ref().map(|reviewer| reviewer.id) {
            return;
        }
        let pending = pending.map(|(id, meta)| (id, meta.clone()));
        self.reviewer = pending.map(|(id, meta)| Reviewer::new(id, meta, cx));
        if self.reviewer.is_none() {
            cx.emit(Dismissed);
        }
        cx.notify();
    }

    fn on_reviewer_event(&mut self, id: ElicitationId, event: &MarkdownReviewEvent, cx: &mut Context<Self>) {
        let reply = match event {
            MarkdownReviewEvent::Submit(submission) => ElicitationAnswer::accept_strings(answer(submission).fields()),
            MarkdownReviewEvent::CopyFormatted(text) => {
                return cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            }
            MarkdownReviewEvent::Cancel => ElicitationAnswer::Cancel,
        };
        Thread::with_core(&self.thread, cx, |core| core.answer_elicitation(id, reply));
    }
}

impl EventEmitter<Dismissed> for ArtifactReview {}

impl Render for ArtifactReview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "artifact-review".to_owned())
            .size_full()
            .children(self.reviewer.as_ref().map(|reviewer| reviewer.view.clone()))
    }
}

impl Reviewer {
    fn new(id: ElicitationId, meta: ArtifactReviewElicitationMeta, cx: &mut Context<ArtifactReview>) -> Self {
        let ArtifactReviewElicitationMeta { path, title, markdown, .. } = meta;
        let document =
            MarkdownDocument::parse_with_metadata(path.map(|path| path.display().to_string()), Some(title), markdown);
        let theme = DesktopTheme::global(cx).review().clone();
        let view = cx.new(|cx| {
            let options = MarkdownReviewerOptions { themes: false, ..MarkdownReviewerOptions::default() };
            let mut reviewer = MarkdownReviewer::with_options(Arc::new(document), theme, options);
            reviewer
                .set_capabilities(ReviewCapabilities { submit: true, clipboard: true, ..ReviewCapabilities::NONE }, cx);
            reviewer
        });
        let subscription =
            cx.subscribe(&view, move |this, _, event: &MarkdownReviewEvent, cx| this.on_reviewer_event(id, event, cx));
        Self { id, view, _subscription: subscription }
    }
}

fn answer(submission: &MarkdownReviewSubmission) -> ArtifactReviewSubmission {
    if submission.decision == MarkdownReviewDecision::Approved && submission.comments.is_empty() {
        ArtifactReviewSubmission::Approved
    } else {
        ArtifactReviewSubmission::Feedback { feedback: submission.formatted.clone() }
    }
}
