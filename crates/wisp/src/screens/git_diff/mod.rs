use std::sync::Arc;

use clankerdiff_ratatui::diff::InteractionPhase;
use clankerdiff_ratatui::theme::ThemeChoice;
use clankerdiff_ratatui::{DiffReviewState, DiffReviewWidget, InputOutcome, default_diff_keybindings};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, StatefulWidget, Widget},
};

use crate::command::GitReviewCommand;
use crate::git_review::{ClientState, DiffReviewEvent, DiffSnapshot, ReviewCapabilities};
use crate::renderer::DrawContext;
use crate::screens::reviewer::{crossterm_event, theme_selection};
use crate::settings::builtin_review_theme_choices;
use crate::surfaces::input::{GitReviewOutput, ReviewOutcome, UiEvent};
use crate::view::generation::Generation;

pub struct GitDiffScreen {
    installed: Option<Arc<DiffSnapshot>>,
    state: DiffReviewState,
    theme_generation: Option<Generation>,
}

impl GitDiffScreen {
    pub fn new() -> Self {
        let mut state = DiffReviewState::loading();
        let mut bindings = default_diff_keybindings();
        bindings.retain(|binding| binding.key != KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL).into());
        state.set_keybindings(bindings);
        state.set_theme_choices(builtin_review_theme_choices());
        state.set_capabilities(ReviewCapabilities { clipboard: false, ..ReviewCapabilities::default() });

        Self { installed: None, state, theme_generation: None }
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

    pub fn install(&mut self, current: &ClientState) {
        self.state.apply_client_state(current, &mut self.installed);
    }

    pub fn on_action_result(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.state.clear_repository_pending(),
            Err(error) => self.state.set_repository_error(error),
        }
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        if self.theme_generation != Some(cx.theme_generation) {
            self.state.set_theme(cx.theme.review().clone());
            self.theme_generation = Some(cx.theme_generation);
        }
        Clear.render(area, buf);
        let title = format!("Git Diff · {:?}", self.state.scope());
        DiffReviewWidget::new().title(title).render(area, buf, &mut self.state);
        self.state.cursor_position()
    }

    fn handle_event(&mut self, event: Event) -> Vec<GitReviewOutput> {
        let outcome = clankerdiff_ratatui::handle_crossterm_event(&mut self.state, event);
        match outcome {
            InputOutcome::Ignored
            | InputOutcome::Consumed
            | InputOutcome::Emitted(DiffReviewEvent::CopyFormattedReview(_)) => Vec::new(),
            InputOutcome::ThemeSelected(id) => vec![GitReviewOutput::SetTheme(theme_selection(id))],
            InputOutcome::Emitted(DiffReviewEvent::Cancel) => {
                vec![GitReviewOutput::Outcome(ReviewOutcome::Cancelled)]
            }
            InputOutcome::Emitted(DiffReviewEvent::SubmitReview(submission)) => {
                vec![GitReviewOutput::Outcome(ReviewOutcome::Submitted(submission.formatted))]
            }
            InputOutcome::Emitted(event) => {
                if let DiffReviewEvent::SetScope(scope) = &event {
                    self.state.set_scope(*scope);
                }
                self.state.set_repository_pending();
                vec![GitReviewOutput::Command(GitReviewCommand::Event(event))]
            }
        }
    }
}

impl Default for GitDiffScreen {
    fn default() -> Self {
        Self::new()
    }
}
