use crate::theme::Theme;
use agent_client_protocol::schema::{PlanEntry, PlanEntryStatus};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const CHECKBOX_EMPTY: &str = "\u{2610}";
const CHECKBOX_FILLED: &str = "\u{2611}";
const SQUARE_FILLED: &str = "\u{25A0}";

pub fn render_plan_lines(entries: &[PlanEntry], theme: &Theme, padding: usize) -> Vec<Line<'static>> {
    if entries.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::with_capacity(entries.len() + 2);

    let blank_prefix = " ".repeat(padding);
    lines.push(Line::from(Span::raw(blank_prefix.clone())));

    let header_style = Style::new().fg(theme.muted);
    lines.push(Line::from(vec![Span::raw(blank_prefix.clone()), Span::styled("Plan", header_style)]));

    for entry in entries {
        let mut spans = vec![Span::raw(blank_prefix.clone())];
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
    use agent_client_protocol::schema::{PlanEntryPriority, PlanEntryStatus};

    fn test_theme() -> Theme {
        Theme::default()
    }

    fn entry(content: &str, status: PlanEntryStatus) -> PlanEntry {
        PlanEntry::new(content.to_string(), PlanEntryPriority::Medium, status)
    }

    #[test]
    fn empty_entries_render_nothing() {
        let theme = test_theme();
        let lines = render_plan_lines(&[], &theme, 4);
        assert!(lines.is_empty());
    }

    #[test]
    fn renders_header_and_entries() {
        let theme = test_theme();
        let entries = vec![
            entry("Research", PlanEntryStatus::Completed),
            entry("Implement", PlanEntryStatus::InProgress),
            entry("Test", PlanEntryStatus::Pending),
        ];
        let lines = render_plan_lines(&entries, &theme, 4);
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn completed_entry_has_checkbox_and_strikethrough() {
        let theme = test_theme();
        let entries = vec![entry("Done task", PlanEntryStatus::Completed)];
        let lines = render_plan_lines(&entries, &theme, 4);

        let line = &lines[2];
        assert_eq!(line.spans.len(), 3);

        let checkbox_span = &line.spans[1];
        assert!(checkbox_span.content.contains(CHECKBOX_FILLED));

        let text_span = &line.spans[2];
        assert!(text_span.content.contains("Done task"));
        assert!(text_span.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn in_progress_entry_has_filled_square() {
        let theme = test_theme();
        let entries = vec![entry("Working", PlanEntryStatus::InProgress)];
        let lines = render_plan_lines(&entries, &theme, 4);

        let checkbox_span = &lines[2].spans[1];
        assert!(checkbox_span.content.contains(SQUARE_FILLED));
    }

    #[test]
    fn in_progress_marker_uses_info_color() {
        let theme = test_theme();
        let entries = vec![entry("Working", PlanEntryStatus::InProgress)];
        let lines = render_plan_lines(&entries, &theme, 4);

        let marker_span = &lines[2].spans[1];
        assert_eq!(marker_span.style.fg, Some(theme.info));
    }

    #[test]
    fn pending_entry_has_empty_checkbox() {
        let theme = test_theme();
        let entries = vec![entry("Todo", PlanEntryStatus::Pending)];
        let lines = render_plan_lines(&entries, &theme, 4);

        let checkbox_span = &lines[2].spans[1];
        assert!(checkbox_span.content.contains(CHECKBOX_EMPTY));
    }

    #[test]
    fn pending_entry_uses_muted_color() {
        let theme = test_theme();
        let entries = vec![entry("Todo", PlanEntryStatus::Pending)];
        let lines = render_plan_lines(&entries, &theme, 4);

        let marker_span = &lines[2].spans[1];
        assert_eq!(marker_span.style.fg, Some(theme.muted));
    }

    #[test]
    fn header_uses_muted_color() {
        let theme = test_theme();
        let entries = vec![entry("Task", PlanEntryStatus::Pending)];
        let lines = render_plan_lines(&entries, &theme, 4);

        let header_span = &lines[1].spans[1];
        assert_eq!(header_span.style.fg, Some(theme.muted));
        assert!(header_span.content.contains("Plan"));
    }

    #[test]
    fn entries_honor_padding() {
        let theme = test_theme();
        let entries = vec![entry("Task", PlanEntryStatus::InProgress)];
        let lines = render_plan_lines(&entries, &theme, 8);

        for line in &lines {
            if line.spans.is_empty() {
                continue;
            }
            assert!(line.spans[0].content.starts_with("        "));
        }
    }

    #[test]
    fn all_statuses_render_distinctly() {
        let theme = test_theme();
        let entries = vec![
            entry("Active", PlanEntryStatus::InProgress),
            entry("Todo", PlanEntryStatus::Pending),
            entry("Done", PlanEntryStatus::Completed),
        ];
        let lines = render_plan_lines(&entries, &theme, 4);

        assert!(lines[2].spans[1].content.contains(SQUARE_FILLED));
        assert!(lines[3].spans[1].content.contains(CHECKBOX_EMPTY));
        assert!(lines[4].spans[1].content.contains(CHECKBOX_FILLED));
    }
}
