use agent_client_protocol::schema::{PlanEntry, PlanEntryStatus};

use tui::{FitOptions, Frame, Line, Style, ViewContext};

const CHECKBOX_EMPTY: &str = "\u{2610}"; // Ballot Box
const CHECKBOX_FILLED: &str = "\u{2611}"; // Ballot Box with Check
const SQUARE_FILLED: &str = "\u{25A0}"; // Black Square

/// Renders the agent's task plan as a compact checklist.
///
/// ```text
/// Plan
///   ☑ ~~Research AI agent patterns~~
///   ■ Implement task tracking
///   ☐ Write integration tests
/// ```
pub struct PlanView<'a> {
    pub entries: &'a [PlanEntry],
}

impl PlanView<'_> {
    pub fn render(&self, context: &ViewContext) -> Frame {
        if self.entries.is_empty() {
            return Frame::empty();
        }

        let mut lines = Vec::with_capacity(self.entries.len() + 2);
        lines.push(Line::default());

        let mut header = Line::default();
        header.push_styled("Plan".to_string(), context.theme.muted());
        lines.push(header);

        for entry in self.entries {
            let mut line = Line::default();
            match entry.status {
                PlanEntryStatus::Completed => {
                    line.push_styled(format!("  {CHECKBOX_FILLED} "), context.theme.muted());
                    let completed_style = Style::fg(context.theme.muted()).strikethrough();
                    line.push_with_style(entry.content.clone(), completed_style);
                }
                PlanEntryStatus::InProgress => {
                    line.push_styled(format!("  {SQUARE_FILLED} "), context.theme.info());
                    line.push_text(entry.content.clone());
                }
                _ => {
                    line.push_styled(format!("  {CHECKBOX_EMPTY} "), context.theme.muted());
                    line.push_styled(entry.content.clone(), context.theme.muted());
                }
            }
            lines.push(line);
        }

        Frame::new(lines).fit(context.size.width, FitOptions::wrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{PlanEntry, PlanEntryPriority, PlanEntryStatus};

    fn ctx() -> ViewContext {
        ViewContext::new((80, 24))
    }

    fn entry(content: &str, status: PlanEntryStatus) -> PlanEntry {
        PlanEntry::new(content.to_string(), PlanEntryPriority::Medium, status)
    }

    #[test]
    fn empty_entries_render_nothing() {
        let view = PlanView { entries: &[] };
        assert!(view.render(&ctx()).lines().is_empty());
    }

    #[test]
    fn renders_header_plus_entries() {
        let entries = vec![
            entry("Research", PlanEntryStatus::Completed),
            entry("Implement", PlanEntryStatus::InProgress),
            entry("Test", PlanEntryStatus::Pending),
        ];
        let view = PlanView { entries: &entries };
        let frame = view.render(&ctx());
        let lines = frame.lines();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].plain_text(), "");
        assert_eq!(lines[1].plain_text(), "Plan");
    }

    #[test]
    fn completed_entry_has_filled_checkbox() {
        let entries = vec![entry("Done task", PlanEntryStatus::Completed)];
        let view = PlanView { entries: &entries };
        let frame = view.render(&ctx());
        let text = frame.lines()[2].plain_text();
        assert!(text.contains(CHECKBOX_FILLED));
        assert!(text.contains("Done task"));
    }

    #[test]
    fn completed_entry_has_strikethrough() {
        let entries = vec![entry("Done task", PlanEntryStatus::Completed)];
        let view = PlanView { entries: &entries };
        let frame = view.render(&ctx());
        let spans = frame.lines()[2].spans();
        let text_span = &spans[1];
        assert!(text_span.style().strikethrough);
    }

    #[test]
    fn in_progress_entry_has_filled_square() {
        let entries = vec![entry("Working", PlanEntryStatus::InProgress)];
        let view = PlanView { entries: &entries };
        let frame = view.render(&ctx());
        let text = frame.lines()[2].plain_text();
        assert!(text.contains(SQUARE_FILLED));
        assert!(text.contains("Working"));
    }

    #[test]
    fn in_progress_marker_uses_info_theme_color() {
        let context = ctx();
        let entries = vec![entry("Working", PlanEntryStatus::InProgress)];
        let view = PlanView { entries: &entries };
        let frame = view.render(&context);
        let spans = frame.lines()[2].spans();
        assert_eq!(spans[0].style().fg, Some(context.theme.info()));
    }

    #[test]
    fn pending_entry_has_empty_checkbox() {
        let entries = vec![entry("Todo", PlanEntryStatus::Pending)];
        let view = PlanView { entries: &entries };
        let frame = view.render(&ctx());
        let text = frame.lines()[2].plain_text();
        assert!(text.contains(CHECKBOX_EMPTY));
        assert!(text.contains("Todo"));
    }
}
