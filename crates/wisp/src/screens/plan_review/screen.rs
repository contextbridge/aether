use crate::surfaces::elicitation::ElicitationResponder;
use crate::surfaces::input::{MouseAction, PlanReviewOutput, ReviewOutcome, UiEvent};
use crate::view::generation::Generation;
use crate::{renderer::DrawContext, settings::builtin_review_theme_choices};
use clankerdiff_core::ReviewCapabilities;
use clankerdiff_markdown::{MarkdownDocument, MarkdownReviewDecision, MarkdownReviewEvent};
use clankerdiff_ratatui::{InputOutcome, MarkdownReviewState, MarkdownReviewWidget};
use crossterm::event::{Event, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    widgets::{Paragraph, StatefulWidget, Widget},
};
use std::sync::Arc;
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};

pub struct PlanReviewScreen {
    title: String,
    state: MarkdownReviewState,
    responder: Option<ElicitationResponder>,
    error: Option<String>,
    theme_generation: Option<Generation>,
}

impl PlanReviewScreen {
    pub fn new(meta: PlanReviewElicitationMeta, responder: impl Into<ElicitationResponder>) -> Self {
        let document =
            MarkdownDocument::parse_with_metadata(Some(meta.plan_path), Some(meta.title.clone()), meta.markdown);
        let mut state = MarkdownReviewState::new(Arc::new(document));
        state.set_theme_choices(builtin_review_theme_choices());
        state.set_capabilities(ReviewCapabilities {
            repository: false,
            refresh: false,
            scope: false,
            submit: true,
            clipboard: false,
        });
        Self { title: meta.title, state, responder: Some(responder.into()), error: None, theme_generation: None }
    }

    pub fn set_theme_choices(&mut self, choices: Vec<clankerdiff_ratatui::ThemeChoice>) {
        self.state.set_theme_choices(choices);
    }

    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<PlanReviewOutput> {
        match event {
            UiEvent::Key(key) => self.on_key(key),
            UiEvent::Paste(text) => self.handle_event(Event::Paste(text)),
            UiEvent::Mouse(action, (column, row)) => self.on_mouse(action, row, column),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<PlanReviewOutput> {
        self.handle_event(Event::Key(key))
    }

    pub fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) -> Vec<PlanReviewOutput> {
        let kind = match action {
            MouseAction::ScrollUp => MouseEventKind::ScrollUp,
            MouseAction::ScrollDown => MouseEventKind::ScrollDown,
            MouseAction::Click => MouseEventKind::Down(MouseButton::Left),
        };
        self.handle_event(Event::Mouse(MouseEvent { kind, row, column, modifiers: KeyModifiers::NONE }))
    }

    pub fn cancel(&mut self) {
        if let Some(mut responder) = self.responder.take() {
            responder.cancel();
        }
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        if self.theme_generation != Some(cx.theme_generation) {
            self.state.set_theme(cx.theme.review().clone());
            self.theme_generation = Some(cx.theme_generation);
        }
        MarkdownReviewWidget::new().title(&self.title).render(area, buf, &mut self.state);
        if let Some(error) = &self.error {
            let footer = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, area.height.min(1));
            Paragraph::new(error.as_str())
                .style(Style::new().fg(cx.theme.error).bg(cx.theme.background))
                .render(footer, buf);
        }
        self.state.cursor_position()
    }

    fn handle_event(&mut self, event: Event) -> Vec<PlanReviewOutput> {
        if self.responder.is_none() {
            return Vec::new();
        }
        let outcome = match clankerdiff_ratatui::handle_markdown_crossterm_event(&mut self.state, event) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.error = Some(error.to_string());
                return Vec::new();
            }
        };
        match outcome {
            InputOutcome::Ignored | InputOutcome::Consumed => Vec::new(),
            InputOutcome::ThemeSelected(id) => vec![PlanReviewOutput::SetTheme(match id {
                clankerdiff_theme::ThemeId::Custom(name) => format!("file:{name}"),
                id => format!("builtin:{id}"),
            })],
            InputOutcome::Emitted(MarkdownReviewEvent::CopyFormatted(_)) => {
                self.error = Some("Clipboard is disabled".into());
                Vec::new()
            }
            InputOutcome::Emitted(MarkdownReviewEvent::Cancel) => {
                self.cancel();
                vec![PlanReviewOutput::Outcome(ReviewOutcome::Cancelled)]
            }
            InputOutcome::Emitted(MarkdownReviewEvent::Submit(submission)) => {
                let decision = match submission.decision {
                    MarkdownReviewDecision::Approved => PlanReviewDecision::Approve,
                    MarkdownReviewDecision::ChangesRequested => PlanReviewDecision::Deny,
                };
                if let Some(mut responder) = self.responder.take() {
                    responder.accept_strings([("decision", decision.as_str()), ("feedback", &submission.formatted)]);
                }
                vec![PlanReviewOutput::Outcome(ReviewOutcome::Submitted(submission.formatted))]
            }
        }
    }
}
