use std::{path::PathBuf, sync::Arc};

use clankerdiff_git::RepositorySnapshot;
use clankerdiff_ratatui::diff::{DiffReviewEvent, DiffScope, InteractionPhase, ReviewCapabilities};
use clankerdiff_ratatui::theme::ThemeChoice;
use clankerdiff_ratatui::{DiffReviewState, DiffReviewWidget, InputOutcome, default_diff_keybindings};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, StatefulWidget, Widget},
};

use crate::git_review::{GitDiffEvent, GitWatchEvent};
use crate::renderer::DrawContext;
use crate::request::RequestId;
use crate::screens::reviewer::{crossterm_event, theme_selection};
use crate::surfaces::input::{GitReviewOutput, ReviewOutcome, UiEvent};
use crate::view::generation::Generation;
use crate::{
    command::{GitCommand, GitWatchCommand},
    settings::builtin_review_theme_choices,
};

pub struct GitDiffScreen {
    review_id: RequestId,
    installed: Option<Arc<RepositorySnapshot>>,
    scope: DiffScope,
    state: DiffReviewState,
    theme_generation: Option<Generation>,
}

impl GitDiffScreen {
    pub fn new(working_dir: PathBuf) -> (Self, GitWatchCommand) {
        let mut state = DiffReviewState::loading();
        let mut bindings = default_diff_keybindings();
        bindings.retain(|binding| binding.key != KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL).into());
        state.set_keybindings(bindings);
        state.set_theme_choices(builtin_review_theme_choices());
        state.set_capabilities(ReviewCapabilities { clipboard: false, ..ReviewCapabilities::default() });
        state.set_repository_pending();

        let review_id = RequestId::next();
        let scope = DiffScope::default();
        let screen = Self { review_id, installed: None, scope, state, theme_generation: None };
        (screen, GitWatchCommand::Open { review_id, working_dir, scope })
    }

    pub fn close(&self) -> GitWatchCommand {
        GitWatchCommand::Close { review_id: self.review_id }
    }

    pub fn set_theme_choices(&mut self, choices: Vec<ThemeChoice>) {
        self.state.set_theme_choices(choices);
    }

    pub(crate) fn is_browsing(&self) -> bool {
        self.state.interaction_phase() == InteractionPhase::Browse
    }

    pub fn on_ui_event(&mut self, event: UiEvent) -> Vec<GitReviewOutput> {
        self.handle_event(crossterm_event(event))
    }

    pub fn on_watch_event(&mut self, event: GitWatchEvent) {
        if event.review_id != self.review_id {
            return;
        }
        match event.result {
            Ok(retained) => {
                if retained.snapshot.scope != self.scope {
                    return;
                }
                if self.installed.as_ref() != Some(&retained.snapshot) {
                    self.state.set_scope(retained.snapshot.scope);
                    self.state.set_document(retained.snapshot.document.clone());
                    self.installed = Some(retained.snapshot.clone());
                }
                self.state.set_background_error(retained.error_message());
            }
            Err(error) => {
                self.state.clear_repository_pending();
                if self.installed.is_some() {
                    self.state.set_background_error(Some(error.to_string()));
                } else {
                    self.state.set_error(error.to_string());
                }
            }
        }
    }

    pub fn on_event(&mut self, event: GitDiffEvent) {
        if event.review_id == self.review_id {
            match event.result {
                Ok(()) => self.state.clear_repository_pending(),
                Err(error) => self.state.set_repository_error(error.to_string()),
            }
        }
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        if self.theme_generation != Some(cx.theme_generation) {
            self.state.set_theme(cx.theme.review().clone());
            self.theme_generation = Some(cx.theme_generation);
        }
        Clear.render(area, buf);
        DiffReviewWidget::new().title(format!("Git Diff · {:?}", self.scope)).render(area, buf, &mut self.state);
        self.state.cursor_position()
    }

    fn refresh(&mut self) -> GitWatchCommand {
        self.state.set_repository_pending();
        GitWatchCommand::Refresh { review_id: self.review_id, scope: self.scope }
    }

    fn handle_event(&mut self, event: Event) -> Vec<GitReviewOutput> {
        let outcome = clankerdiff_ratatui::handle_crossterm_event(&mut self.state, event);
        match outcome {
            InputOutcome::Ignored | InputOutcome::Consumed => Vec::new(),
            InputOutcome::ThemeSelected(id) => vec![GitReviewOutput::SetTheme(theme_selection(id))],
            InputOutcome::Emitted(event) => match event {
                DiffReviewEvent::Cancel => vec![GitReviewOutput::Outcome(ReviewOutcome::Cancelled)],
                DiffReviewEvent::SubmitReview(submission) => {
                    vec![GitReviewOutput::Outcome(ReviewOutcome::Submitted(submission.formatted))]
                }
                DiffReviewEvent::RepositoryAction(action) => {
                    self.state.set_repository_pending();
                    vec![GitReviewOutput::Task(GitCommand::Apply { review_id: self.review_id, action })]
                }
                DiffReviewEvent::SetScope(scope) => {
                    self.scope = scope;
                    self.state.set_scope(scope);
                    vec![GitReviewOutput::Watch(self.refresh())]
                }
                DiffReviewEvent::Refresh => vec![GitReviewOutput::Watch(self.refresh())],
                DiffReviewEvent::CopyFormattedReview(_) => Vec::new(),
            },
        }
    }
}
