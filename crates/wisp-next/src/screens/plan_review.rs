use acp_utils::notifications::{ElicitationAction, ElicitationResponse};
use agent_client_protocol::Responder;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};

use crate::theme::Theme;

pub struct PlanReviewScreen {
    title: String,
    lines: Vec<String>,
    offset: usize,
    responder: Option<Responder<ElicitationResponse>>,
}

impl PlanReviewScreen {
    pub fn new(meta: PlanReviewElicitationMeta, responder: Responder<ElicitationResponse>) -> Self {
        Self {
            title: meta.title,
            lines: meta.markdown.lines().map(str::to_string).collect(),
            offset: 0,
            responder: Some(responder),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.respond(ElicitationAction::Cancel, None);
                true
            }
            KeyCode::Char('a') => {
                self.respond(ElicitationAction::Accept, Some(PlanReviewDecision::Approve.response_content(None)));
                true
            }
            KeyCode::Char('r') => {
                self.respond(
                    ElicitationAction::Accept,
                    Some(PlanReviewDecision::Deny.response_content(Some("Plan needs changes."))),
                );
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.offset = self.offset.saturating_sub(1);
                false
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.offset = self.offset.saturating_add(1).min(self.lines.len().saturating_sub(1));
                false
            }
            _ => false,
        }
    }

    pub fn render(&self, frame: &mut Frame, theme: &Theme) {
        let area = frame.area();
        let visible = usize::from(area.height.saturating_sub(3));
        let mut rendered: Vec<Line<'static>> =
            self.lines.iter().skip(self.offset).take(visible).cloned().map(Line::raw).collect();
        rendered.push(Line::styled(
            "j/k scroll · a approve · r request changes · Esc cancel",
            Style::new().fg(theme.muted),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.title))
            .border_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD));
        frame.render_widget(Paragraph::new(Text::from(rendered)).block(block).wrap(Wrap { trim: false }), area);
    }

    pub fn cancel(&mut self) {
        self.respond(ElicitationAction::Cancel, None);
    }

    fn respond(&mut self, action: ElicitationAction, content: Option<serde_json::Value>) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(ElicitationResponse { action, content });
        }
    }
}
