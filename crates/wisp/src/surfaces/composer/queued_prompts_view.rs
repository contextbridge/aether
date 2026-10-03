use crate::app::QueuedPrompt;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

const MAX_PROMPT_ROWS: usize = 3;

pub struct QueuedPromptsView<'a> {
    prompts: &'a [QueuedPrompt],
    theme: &'a Theme,
}

impl<'a> QueuedPromptsView<'a> {
    pub fn new(prompts: &'a [QueuedPrompt], theme: &'a Theme) -> Self {
        Self { prompts, theme }
    }

    pub fn line_count(&self) -> usize {
        self.prompts.len().min(MAX_PROMPT_ROWS + 1)
    }
}

impl Widget for QueuedPromptsView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.prompts.is_empty() || area.height == 0 {
            return;
        }
        let label = Style::new().fg(self.theme.muted);
        let text = Style::new().fg(self.theme.text_secondary);
        let shown = if self.prompts.len() > MAX_PROMPT_ROWS + 1 { MAX_PROMPT_ROWS } else { self.prompts.len() };
        let mut lines: Vec<Line<'static>> = self.prompts[..shown]
            .iter()
            .map(|prompt| Line::from(vec![Span::styled("queued › ", label), Span::styled(summary(&prompt.submission.text), text)]))
            .collect();
        if shown < self.prompts.len() {
            lines.push(Line::styled(format!("+{} more queued", self.prompts.len() - shown), label));
        }
        Paragraph::new(lines).render(area, buf);
    }
}

fn summary(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    if lines.next().is_some() { format!("{first} …") } else { first.to_string() }
}
