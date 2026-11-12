use crate::threads::{Thread, ThreadEvent};
use crate::ui::{ActiveTheme, IconName, empty_state};
use clankerdiff_core::{DiffReviewCommand, DiffReviewEvent, DiffSide, FilesMode, RepoPath, SourcePath, TargetLocation};
use clankerdiff_rouge::{DiffClientView, DiffViewerOptions, ReviewHandle, ReviewOutput};
use clankerdiff_rouge_shell::rouge::core::{Styled, View, WindowHandle, div, dynamic, view, window_handle};
use clankerdiff_rouge_shell::rouge::reactive::{Callback, Memo, Scope, Signal, effect, memo, signal, untrack};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) struct Files {
    thread: Thread,
    client: Signal<Option<DiffClientView>>,
    comments: Memo<usize>,
    on_dismissed: Callback,
    window: WindowHandle,
    scope: Scope,
}

impl Files {
    pub(crate) fn new(thread: Thread, on_dismissed: impl Fn() + 'static) -> Self {
        let client: Signal<Option<DiffClientView>> = signal(None);
        let comments = memo(move || {
            client.get().map_or(0, |client| {
                client.review().comments_revision().get();
                client.review().with(|review| review.session().review().len())
            })
        });
        let files = Self {
            thread,
            client,
            comments,
            on_dismissed: Callback::from(on_dismissed),
            window: window_handle(),
            scope: Scope::new(),
        };
        let review = memo(move || thread.review().map(|state| Arc::as_ptr(&state) as usize));
        effect(move || {
            review.get();
            untrack(|| files.sync());
        });
        effect(move || {
            let theme = ActiveTheme::current().review();
            untrack(|| {
                if let Some(client) = files.client.get_untracked() {
                    client.review().set_theme(theme);
                }
            });
        });
        thread.on_event(move |event| files.on_thread_event(&event));
        files
    }

    pub(crate) fn review(self) -> Option<ReviewHandle> {
        self.client.get_untracked().map(|client| *client.review())
    }

    pub(crate) fn comments(self) -> usize {
        self.comments.get()
    }

    pub(crate) fn submit(self) {
        self.run(DiffReviewCommand::SubmitReview);
    }

    pub(crate) fn copy(self) {
        self.run(DiffReviewCommand::CopyReview);
    }

    pub(crate) fn open(self, path: RepoPath, line: Option<u32>) {
        self.run(DiffReviewCommand::SetFilesMode(FilesMode::Diff));
        let line_number = line.map_or(1, |line| line.max(1) as usize);
        let target = TargetLocation { path: SourcePath::Repo(path), side: DiffSide::New, line_number };
        self.run(DiffReviewCommand::JumpTo(target));
    }

    pub(crate) fn view(self) -> impl View {
        let thread = self.thread;
        let waiting = memo(move || {
            let status = thread.status();
            let title = if status.is_stopped() { "Agent not running" } else { "Waiting for the agent" };
            (title, thread.listed_files().err().unwrap_or_default())
        });
        let client = self.client;
        let shown = memo(move || client.with(Option::is_some));
        dynamic(move || {
            if shown.get() {
                let client = untrack(|| client.get_untracked());
                return view! { <div test_id="changes-viewer" size_full>{client.map(|client| client.view())}</div> }
                    .into_any();
            }
            let (title, detail) = waiting.get();
            view! {
                <div test_id="files-status" size_full flex items_center justify_center>
                    <empty_state icon=IconName::FileDiff title detail/>
                </div>
            }
            .into_any()
        })
    }

    fn run(self, command: DiffReviewCommand) {
        if let Some(client) = self.client.get_untracked() {
            let _ = client.handle_command(command);
        }
    }

    fn sync(self) {
        let Some(state) = self.thread.review() else {
            self.client.set(None);
            return;
        };
        let client = self.client.get_untracked().unwrap_or_else(|| {
            let theme = ActiveTheme::current().review();
            let options = DiffViewerOptions { themes: false, review_bar: false, ..DiffViewerOptions::default() };
            let client = self.scope.run(|| DiffClientView::mount(theme, options, move |output| self.on_output(output)));
            self.client.set(Some(client));
            client
        });
        client.set_state(&state);
    }

    fn on_output(self, output: ReviewOutput) {
        let ReviewOutput::Event(event) = output else {
            return;
        };
        match event {
            DiffReviewEvent::SubmitReview(submission) => {
                self.thread.update_core(|core| core.submit_review(&submission.formatted));
            }
            DiffReviewEvent::Cancel => self.on_dismissed.call(),
            DiffReviewEvent::CopyFormattedReview(text) => self.window.copy_text(text),
            event => self.thread.update_core(|core| core.send_review_event(event)),
        }
    }

    fn on_thread_event(self, event: &ThreadEvent) {
        let Some(client) = self.client.get_untracked() else {
            return;
        };
        match event {
            ThreadEvent::ReviewActionFinished(result) => {
                client.command_finished(result.clone().map_err(|error| error.to_string()));
            }
            ThreadEvent::ReviewLookupFinished(reply) => client.lookup_finished(reply.clone()),
            ThreadEvent::ReviewSubmitted => client.review().clear_review(),
            _ => {}
        }
    }
}
