use crate::theme::Theme;
use agent_client_protocol::schema::v1::{PlanEntry, PlanEntryStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

const CHECKBOX_EMPTY: &str = "\u{2610}";
const CHECKBOX_FILLED: &str = "\u{2611}";
const SQUARE_FILLED: &str = "\u{25A0}";

/// Renders the plan checklist as a ratatui widget. Callers inset the area they
/// pass to [`Widget::render`] rather than asking for an indent.
pub struct PlanView<'a> {
    entries: &'a [PlanEntry],
    theme: &'a Theme,
}

impl<'a> PlanView<'a> {
    pub fn new(entries: &'a [PlanEntry], theme: &'a Theme) -> Self {
        Self { entries, theme }
    }

    /// Number of lines the plan view will occupy (header + blank + entries).
    pub fn line_count(&self) -> usize {
        if self.entries.is_empty() { 0 } else { self.entries.len() + 2 }
    }
}

impl Widget for PlanView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.entries.is_empty() || area.height == 0 {
            return;
        }
        let lines = build_plan_lines(self.entries, self.theme);
        Paragraph::new(lines).render(area, buf);
    }
}

fn build_plan_lines(entries: &[PlanEntry], theme: &Theme) -> Vec<Line<'static>> {
    let header_style = Style::new().fg(theme.muted);

    let mut lines = Vec::with_capacity(entries.len() + 2);
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Plan", header_style)));

    for entry in entries {
        let mut spans = Vec::new();
        match entry.status {
            PlanEntryStatus::Completed => {
                spans.push(Span::styled(format!("  {CHECKBOX_FILLED} "), Style::new().fg(theme.muted)));
                spans.push(Span::styled(
                    entry.content.clone(),
                    Style::new().fg(theme.muted).add_modifier(Modifier::CROSSED_OUT),
                ));
            }
            PlanEntryStatus::InProgress => {
                spans.push(Span::styled(format!("  {SQUARE_FILLED} "), Style::new().fg(theme.info)));
                spans.push(Span::styled(entry.content.clone(), Style::new().fg(theme.text_primary)));
            }
            _ => {
                spans.push(Span::styled(format!("  {CHECKBOX_EMPTY} "), Style::new().fg(theme.muted)));
                spans.push(Span::styled(entry.content.clone(), Style::new().fg(theme.muted)));
            }
        }
        lines.push(Line::from(spans));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{PlanEntryPriority, PlanEntryStatus};

    fn test_theme() -> Theme {
        Theme::default()
    }

    fn entry(content: &str, status: PlanEntryStatus) -> PlanEntry {
        PlanEntry::new(content.to_string(), PlanEntryPriority::Medium, status)
    }

    #[test]
    fn empty_entries_render_nothing() {
        let theme = test_theme();
        assert_eq!(PlanView::new(&[], &theme).line_count(), 0);
    }

    #[test]
    fn line_count_matches_entries_plus_header() {
        let theme = test_theme();
        let entries = vec![entry("Task", PlanEntryStatus::Pending)];
        assert_eq!(PlanView::new(&entries, &theme).line_count(), 3);
        assert_eq!(PlanView::new(&[], &theme).line_count(), 0);
    }

    #[test]
    fn renders_header_and_entries() {
        let theme = test_theme();
        let entries = vec![
            entry("Research", PlanEntryStatus::Completed),
            entry("Implement", PlanEntryStatus::InProgress),
            entry("Test", PlanEntryStatus::Pending),
        ];
        let view = PlanView::new(&entries, &theme);
        assert_eq!(view.line_count(), 5);
    }
}
