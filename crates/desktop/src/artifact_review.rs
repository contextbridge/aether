use crate::platform::Platform;
use crate::threads::Thread;
use crate::ui::ActiveReviewTheme;
use clankerdiff_core::ReviewCapabilities;
use clankerdiff_markdown::{MarkdownDocument, MarkdownReviewDecision, MarkdownReviewEvent, MarkdownReviewSubmission};
use clankerdiff_rouge::{MarkdownController, MarkdownHandle, MarkdownOutput, markdown_review_view};
use client_core::elicitation::{ElicitationAnswer, ElicitationId};
use rouge::core::{Styled, View, WindowHandle, div, dynamic, view, window_handle};
use rouge::reactive::{Callback, Scope, Signal, Stored, effect, memo, signal, spawn, stored, untrack};
use std::rc::Rc;
use std::sync::Arc;
use utils::artifact_review::{ArtifactReviewElicitationMeta, ArtifactReviewSubmission};

#[derive(Clone, Copy)]
pub(crate) struct ArtifactReview {
    thread: Thread,
    reviewer: Signal<Option<Reviewer>>,
    on_dismissed: Callback,
    platform: Stored<Rc<dyn Platform>>,
    window: WindowHandle,
    scope: Scope,
}

#[derive(Clone, Copy)]
struct Reviewer {
    id: ElicitationId,
    handle: MarkdownHandle,
    scope: Scope,
}

impl ArtifactReview {
    pub(crate) fn new(thread: Thread, platform: Rc<dyn Platform>, on_dismissed: impl Fn() + 'static) -> Self {
        let review = Self {
            thread,
            reviewer: signal(None),
            on_dismissed: Callback::from(on_dismissed),
            platform: stored(platform),
            window: window_handle(),
            scope: Scope::new(),
        };
        let pending = memo(move || thread.pending_review().map(|(id, _)| id));
        effect(move || {
            let pending = pending.get();
            untrack(|| review.sync(pending));
        });
        effect(move || {
            let theme = ActiveReviewTheme::current().review();
            untrack(|| {
                if let Some(reviewer) = review.reviewer.get_untracked() {
                    reviewer.handle.set_theme(theme);
                }
            });
        });
        review
    }

    pub(crate) fn view(self) -> impl View {
        let shown = memo(move || self.reviewer.get().map(|reviewer| reviewer.id));
        view! {
            <div test_id="artifact-review" size_full>
                <dynamic>{shown.get().and_then(|_| untrack(|| self.reviewer.get_untracked())).map(|reviewer| markdown_review_view(reviewer.handle))}</dynamic>
            </div>
        }
    }

    fn sync(self, pending: Option<ElicitationId>) {
        if pending == self.reviewer.with_untracked(|reviewer| reviewer.map(|reviewer| reviewer.id)) {
            return;
        }
        if let Some(previous) = self.reviewer.get_untracked() {
            spawn(async move { previous.scope.dispose() });
        }
        let meta = self.thread.pending_review().filter(|(id, _)| Some(*id) == pending);
        let reviewer = meta.map(|(id, meta)| self.reviewer(id, meta));
        let dismissed = reviewer.is_none();
        self.reviewer.set(reviewer);
        if dismissed {
            self.on_dismissed.call();
        }
    }

    fn reviewer(self, id: ElicitationId, meta: ArtifactReviewElicitationMeta) -> Reviewer {
        let ArtifactReviewElicitationMeta { path, title, markdown, .. } = meta;
        let document =
            MarkdownDocument::parse_with_metadata(path.map(|path| path.display().to_string()), Some(title), markdown);
        let theme = ActiveReviewTheme::current().review();
        let scope = self.scope.child();
        let handle = scope.run(|| {
            let handle =
                MarkdownController::new(Arc::new(document), theme).mount(move |output| self.on_output(id, output));
            handle.set_capabilities(ReviewCapabilities { submit: true, clipboard: true, ..ReviewCapabilities::NONE });
            handle
        });
        Reviewer { id, handle, scope }
    }

    fn on_output(self, id: ElicitationId, output: MarkdownOutput) {
        let reply = match output {
            MarkdownOutput::Event(MarkdownReviewEvent::Submit(submission)) => {
                ElicitationAnswer::accept_strings(answer(&submission).fields())
            }
            MarkdownOutput::Event(MarkdownReviewEvent::CopyFormatted(text)) | MarkdownOutput::CopyCode(text) => {
                return self.window.copy_text(text);
            }
            MarkdownOutput::Event(MarkdownReviewEvent::Cancel) => ElicitationAnswer::Cancel,
            MarkdownOutput::OpenLink(url) => return self.platform.with(|platform| platform.open(&url)),
            MarkdownOutput::ThemeChanged(_) => return,
        };
        self.thread.update_core(|core| core.answer_elicitation(id, reply));
    }
}

fn answer(submission: &MarkdownReviewSubmission) -> ArtifactReviewSubmission {
    if submission.decision == MarkdownReviewDecision::Approved && submission.comments.is_empty() {
        ArtifactReviewSubmission::Approved
    } else {
        ArtifactReviewSubmission::Feedback { feedback: submission.formatted.clone() }
    }
}
