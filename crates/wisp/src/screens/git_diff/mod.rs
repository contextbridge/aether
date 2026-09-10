use std::path::PathBuf;

use clankerdiff_core::{DiffReviewEvent, DiffScope, InteractionPhase, ReviewCapabilities};
use clankerdiff_ratatui::{DiffReviewState, DiffReviewWidget, InputOutcome, default_diff_keybindings};
use clankerdiff_theme::ThemeChoice;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::StatefulWidget,
};

use crate::git_review::GitDiffEvent;
use crate::renderer::DrawContext;
use crate::request::RequestId;
use crate::screens::reviewer::{crossterm_event, theme_selection};
use crate::surfaces::input::{GitReviewOutput, ReviewOutcome, UiEvent};
use crate::view::generation::Generation;
use crate::{command::GitCommand, settings::builtin_review_theme_choices};

pub struct GitDiffScreen {
    working_dir: PathBuf,
    repo_root: Option<PathBuf>,
    scope: DiffScope,
    state: DiffReviewState,
    pending: Option<(RequestId, RequestKind)>,
    theme_generation: Option<Generation>,
}

impl GitDiffScreen {
    pub fn new(working_dir: PathBuf) -> (Self, GitCommand) {
        let mut state = DiffReviewState::loading();
        let mut bindings = default_diff_keybindings();
        bindings.retain(|binding| binding.key != KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL).into());
        state.set_keybindings(bindings);
        state.set_theme_choices(builtin_review_theme_choices());
        state.set_capabilities(ReviewCapabilities { clipboard: false, ..ReviewCapabilities::default() });

        let mut screen = Self {
            working_dir,
            repo_root: None,
            scope: DiffScope::default(),
            state,
            pending: None,
            theme_generation: None,
        };

        let command = screen.load(false);
        (screen, command)
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

    pub fn on_event(&mut self, event: GitDiffEvent) -> Vec<GitReviewOutput> {
        let Some((id, kind)) = self.pending else {
            return Vec::new();
        };
        if event.request_id() != id {
            return Vec::new();
        }
        match (event, kind) {
            (GitDiffEvent::Loaded { result, .. }, RequestKind::Load { after_action }) => {
                self.pending = None;
                self.state.clear_repository_pending();
                match result {
                    Ok(snapshot) => {
                        self.repo_root = Some(PathBuf::from(&snapshot.document.repo_root));
                        self.state.set_scope(snapshot.scope);
                        self.state.set_document(snapshot.document);
                        self.state.set_background_error(None);
                    }
                    Err(error) if after_action => {
                        self.state.set_error(format!("Repository action succeeded, but refresh failed: {error}"));
                    }
                    Err(error) => self.state.set_error(error.to_string()),
                }
            }
            (GitDiffEvent::ActionFinished { result, .. }, RequestKind::Action) => {
                self.pending = None;
                match result {
                    Ok(()) => return vec![GitReviewOutput::Task(self.load(true))],
                    Err(error) => self.state.set_repository_error(error.to_string()),
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        if self.theme_generation != Some(cx.theme_generation) {
            self.state.set_theme(cx.theme.review().clone());
            self.theme_generation = Some(cx.theme_generation);
        }
        DiffReviewWidget::new().title(format!("Git Diff · {:?}", self.scope)).render(area, buf, &mut self.state);
        self.state.cursor_position()
    }

    fn load(&mut self, after_action: bool) -> GitCommand {
        let request_id = RequestId::next();
        self.pending = Some((request_id, RequestKind::Load { after_action }));
        self.state.set_loading();
        GitCommand::Load { request_id, working_dir: self.working_dir.clone(), scope: self.scope }
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
                DiffReviewEvent::RepositoryAction(action) if self.pending.is_none() => {
                    let Some(repo_root) = self.repo_root.clone() else {
                        return Vec::new();
                    };
                    let request_id = RequestId::next();
                    self.pending = Some((request_id, RequestKind::Action));
                    self.state.set_repository_pending();
                    vec![GitReviewOutput::Task(GitCommand::Apply { request_id, repo_root, action })]
                }
                DiffReviewEvent::SetScope(scope) if self.pending.is_none() => {
                    self.scope = scope;
                    self.state.set_scope(scope);
                    vec![GitReviewOutput::Task(self.load(false))]
                }
                DiffReviewEvent::Refresh if self.pending.is_none() => vec![GitReviewOutput::Task(self.load(false))],
                _ => Vec::new(),
            },
        }
    }
}

#[derive(Clone, Copy)]
enum RequestKind {
    Load { after_action: bool },
    Action,
}
