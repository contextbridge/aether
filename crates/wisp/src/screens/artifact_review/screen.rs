use crate::screens::reviewer::{crossterm_event, theme_selection};
use crate::surfaces::elicitation::ElicitationResponder;
use crate::surfaces::input::{ArtifactReviewOutput, ReviewOutcome, UiEvent};
use crate::surfaces::modal::frame::ModalFrame;
use agent_client_protocol::schema::v2::ElicitationContentValue;
use crate::view::generation::Generation;
use crate::{renderer::DrawContext, settings::builtin_review_theme_choices};
use clankerdiff_ratatui::ReviewCapabilities;
use clankerdiff_ratatui::markdown::{MarkdownDocument, MarkdownReviewEvent};
use clankerdiff_ratatui::{InputOutcome, MarkdownReviewState, MarkdownReviewWidget};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Position, Rect},
    style::Style,
    widgets::{Clear, Paragraph, StatefulWidget, Widget},
};
use std::sync::Arc;
use utils::artifact_review::{ArtifactReviewElicitationMeta, ArtifactReviewSubmission};

pub struct ArtifactReviewScreen {
    title: String,
    state: MarkdownReviewState,
    responder: ElicitationResponder,
    error: Option<String>,
    confirming_approval: bool,
    theme_generation: Option<Generation>,
}

impl ArtifactReviewScreen {
    pub fn new(meta: ArtifactReviewElicitationMeta, responder: impl Into<ElicitationResponder>) -> Self {
        let document = MarkdownDocument::parse_with_metadata(
            Some(meta.path.display().to_string()),
            Some(meta.title.clone()),
            meta.markdown,
        );
        let mut state = MarkdownReviewState::new(Arc::new(document));
        state.set_theme_choices(builtin_review_theme_choices());
        state.set_capabilities(ReviewCapabilities {
            repository: false,
            refresh: false,
            scope: false,
            submit: true,
            clipboard: false,
        });
        Self { title: meta.title, state, responder: responder.into(), error: None, confirming_approval: false, theme_generation: None }
    }

    pub fn set_theme_choices(&mut self, choices: Vec<clankerdiff_ratatui::ThemeChoice>) {
        self.state.set_theme_choices(choices);
    }

    pub fn on_ui_event(&mut self, event: UiEvent) -> Vec<ArtifactReviewOutput> {
        self.handle_event(crossterm_event(event))
    }

    pub fn cancel(&mut self) {
        self.responder.cancel();
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        if self.theme_generation != Some(cx.theme_generation) {
            self.state.set_theme(cx.theme.review().clone());
            self.theme_generation = Some(cx.theme_generation);
        }
        Clear.render(area, buf);
        MarkdownReviewWidget::new().title(&self.title).render(area, buf, &mut self.state);
        if let Some(error) = &self.error {
            let footer = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, area.height.min(1));
            Paragraph::new(error.as_str())
                .style(Style::new().fg(cx.theme.error).bg(cx.theme.background))
                .render(footer, buf);
        }
        if self.confirming_approval {
            let frame = ModalFrame::new(
                "Approve and continue",
                Some("Enter: approve · Esc: return to review".into()),
                Constraint::Length(60),
                Constraint::Length(9),
                cx.theme,
            );
            (&frame).render(area, buf);
            Paragraph::new("Approve this artifact without feedback?")
                .style(Style::new().fg(cx.theme.text_primary))
                .render(frame.inner(area), buf);
            return None;
        }
        self.state.cursor_position()
    }

    fn submit(&mut self, submission: ArtifactReviewSubmission) -> Vec<ArtifactReviewOutput> {
        self.responder.accept(Some(submission.fields().map(|(name, value)| {
            (name.to_string(), ElicitationContentValue::String(value.to_string()))
        }).collect()));
        vec![match submission {
            ArtifactReviewSubmission::Approved => ArtifactReviewOutput::Approved,
            ArtifactReviewSubmission::Feedback { feedback } => ArtifactReviewOutput::Outcome(ReviewOutcome::Submitted(feedback)),
        }]
    }

    fn handle_event(&mut self, event: Event) -> Vec<ArtifactReviewOutput> {
        if self.responder.is_answered() {
            return Vec::new();
        }
        if self.confirming_approval {
            if let Event::Key(key) = event
                && key.kind == KeyEventKind::Press
                && key.modifiers.is_empty()
            {
                match key.code {
                    KeyCode::Enter => return self.submit(ArtifactReviewSubmission::Approved),
                    KeyCode::Esc => self.confirming_approval = false,
                    _ => {}
                }
            }
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
            InputOutcome::ThemeSelected(id) => vec![ArtifactReviewOutput::SetTheme(theme_selection(id))],
            InputOutcome::Emitted(MarkdownReviewEvent::CopyFormatted(_)) => {
                self.error = Some("Clipboard is disabled".into());
                Vec::new()
            }
            InputOutcome::Emitted(MarkdownReviewEvent::Cancel) => {
                self.cancel();
                vec![ArtifactReviewOutput::Outcome(ReviewOutcome::Cancelled)]
            }
            InputOutcome::Emitted(MarkdownReviewEvent::Submit(submission)) => {
                if submission.comments.is_empty() {
                    self.confirming_approval = true;
                    Vec::new()
                } else {
                    self.submit(ArtifactReviewSubmission::Feedback { feedback: submission.formatted })
                }
            }
        }
    }
}
