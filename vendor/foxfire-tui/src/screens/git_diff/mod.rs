use std::sync::Arc;

use clankerdiff_ratatui::diff::InteractionPhase;
use clankerdiff_ratatui::theme::ThemeChoice;
use clankerdiff_ratatui::{DiffReviewState, DiffReviewWidget, InputOutcome};
use crossterm::event::Event;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, StatefulWidget, Widget},
};

use crate::renderer::DrawContext;
use crate::screens::reviewer::{crossterm_event, theme_change};
use crate::settings::builtin_review_theme_choices;
use crate::surfaces::input::UiOutput;
use crate::view::generation::Generation;
use crate::view::input::UiEvent;
use clankerdiff_client::{ClientError, ClientState, DiffReviewEvent, LookupReply};
use clankerdiff_ratatui::ReviewCapabilities;
use clankerdiff_ratatui::diff::DiffDocument;

pub struct GitDiffScreen {
    installed: Option<Arc<DiffDocument>>,
    state: DiffReviewState,
    theme_generation: Option<Generation>,
}

impl GitDiffScreen {
    pub fn new() -> Self {
        let mut state = DiffReviewState::loading();
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

    pub fn on_ui_event(&mut self, event: &UiEvent) -> Vec<UiOutput> {
        self.handle_event(crossterm_event(event))
    }

    pub fn install(&mut self, current: &ClientState) {
        self.state.apply_client_state(current, &mut self.installed);
    }

    pub fn on_action_result(&mut self, result: Result<(), ClientError>) {
        match result {
            Ok(()) => self.state.clear_repository_pending(),
            Err(error) => self.state.set_repository_error(error.to_string()),
        }
    }

    pub fn on_lookup_result(&mut self, reply: LookupReply) -> Vec<UiOutput> {
        self.state.lookup_finished(reply);
        self.lookups()
    }

    pub fn on_tick(&mut self) -> Vec<UiOutput> {
        self.lookups()
    }

    fn lookups(&mut self) -> Vec<UiOutput> {
        self.state
            .take_lookups()
            .into_iter()
            .map(|lookup| UiOutput::ReviewEvent(DiffReviewEvent::Lookup(lookup)))
            .collect()
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

    fn handle_event(&mut self, event: Event) -> Vec<UiOutput> {
        let outcome = clankerdiff_ratatui::handle_crossterm_event(&mut self.state, event);
        let mut outputs = match outcome {
            InputOutcome::Ignored
            | InputOutcome::Consumed
            | InputOutcome::ViewModeSelected(_)
            | InputOutcome::Emitted(DiffReviewEvent::CopyFormattedReview(_) | DiffReviewEvent::Lookup(_)) => Vec::new(),
            InputOutcome::ThemeSelected(id) => vec![theme_change(id)],
            InputOutcome::Emitted(DiffReviewEvent::Cancel) => vec![UiOutput::Close],
            InputOutcome::Emitted(DiffReviewEvent::SubmitReview(submission)) => {
                vec![UiOutput::SubmitReview(submission.formatted)]
            }
            InputOutcome::Emitted(event) => {
                self.state.set_repository_pending();
                vec![UiOutput::ReviewEvent(event)]
            }
        };
        outputs.extend(self.lookups());
        outputs
    }
}

impl Default for GitDiffScreen {
    fn default() -> Self {
        Self::new()
    }
}
