use super::PlanSection;
use crate::components::common::{CachedLayer, VerticalCursor};
use tui::{Component, Event, Frame, KeyCode, Line, Style, Theme, ViewContext, truncate_text};

pub enum OutlinePanelMessage {
    OpenSelectedAnchor(usize),
}

pub struct OutlinePanel {
    sections: Vec<PlanSection>,
    cursor: VerticalCursor,
    cached_rows: CachedLayer<u16, Vec<Line>>,
    focused: bool,
}

impl OutlinePanel {
    pub fn new(sections: Vec<PlanSection>) -> Self {
        Self { sections, cursor: VerticalCursor::new(), cached_rows: CachedLayer::new(), focused: false }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn selected_anchor_line_no(&self) -> Option<usize> {
        self.sections.get(self.cursor.row).map(|section| section.first_line_no)
    }

    fn move_selected(&mut self, delta: isize) {
        self.cursor.move_by(delta, self.sections.len().saturating_sub(1));
    }

    fn move_to_start(&mut self) {
        self.cursor.move_to_start();
    }

    fn move_to_end(&mut self) {
        self.cursor.move_to_end(self.sections.len().saturating_sub(1));
    }

    fn ensure_visible(&mut self, viewport_height: usize) {
        if self.sections.is_empty() {
            self.cursor.scroll = 0;
            return;
        }

        self.cursor.row = self.cursor.row.min(self.sections.len().saturating_sub(1));
        self.cursor.ensure_visible(self.cursor.row, viewport_height);
    }
}

impl Component for OutlinePanel {
    type Message = OutlinePanelMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return None;
        };

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selected(1);
                Some(vec![])
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selected(-1);
                Some(vec![])
            }
            KeyCode::Char('g') => {
                self.move_to_start();
                Some(vec![])
            }
            KeyCode::Char('G') => {
                self.move_to_end();
                Some(vec![])
            }
            KeyCode::Enter => {
                self.selected_anchor_line_no().map(OutlinePanelMessage::OpenSelectedAnchor).map(|message| vec![message])
            }
            _ => None,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let theme = &ctx.theme;
        let width = usize::from(ctx.size.width);
        let height = usize::from(ctx.size.height);
        let body_height = height.saturating_sub(1);

        self.ensure_visible(body_height);

        let sections = &self.sections;
        let cached_section_rows = self.cached_rows.ensure(ctx.size.width, || {
            sections.iter().map(|section| build_section_row(section, width, theme)).collect()
        });
        let cursor_row = self.cursor.row;
        let scroll = self.cursor.scroll;

        let mut lines = Vec::with_capacity(height);

        let mut header = Line::default();
        let title_fg = if self.focused { theme.accent() } else { theme.text_primary() };
        header.push_text(" ");
        header.push_with_style("Outline", Style::fg(title_fg).bold());
        header.extend_bg_to_width(width);
        lines.push(header);

        for row in 0..body_height {
            let section_index = scroll + row;
            if let Some(section) = self.sections.get(section_index) {
                let selected = section_index == cursor_row;
                if selected {
                    lines.push(build_selected_row(section, width, theme));
                } else {
                    lines.push(cached_section_rows[section_index].clone());
                }
            } else {
                let mut line = Line::default();
                line.push_text(" ".repeat(width));
                lines.push(line);
            }
        }

        Frame::new(lines)
    }
}

fn build_section_row(section: &PlanSection, width: usize, _theme: &Theme) -> Line {
    build_row_with_style(section, width, "  ", Style::default())
}

fn build_selected_row(section: &PlanSection, width: usize, theme: &Theme) -> Line {
    build_row_with_style(section, width, "> ", theme.selected_row_style())
}

fn build_row_with_style(section: &PlanSection, width: usize, marker: &str, style: Style) -> Line {
    let indent = "  ".repeat(section.level.saturating_sub(1) as usize);
    let prefix = format!("{marker}{indent}");
    let title_width = width.saturating_sub(prefix.chars().count());
    let title = truncate_text(&section.title, title_width);

    let mut line = Line::default();
    line.push_with_style(prefix, style);
    line.push_with_style(title.as_ref(), style);
    line.extend_bg_to_width(width);
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{Event, KeyEvent, KeyModifiers};

    fn section(title: &str, level: u8, first_line_no: usize) -> PlanSection {
        PlanSection { title: title.to_string(), level, first_line_no }
    }

    #[tokio::test]
    async fn navigation_moves_selection_without_emitting_parent_messages() {
        let mut panel = OutlinePanel::new(vec![section("One", 1, 1), section("Two", 2, 10)]);

        let messages =
            panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))).await.unwrap();

        assert!(messages.is_empty());
        assert_eq!(panel.selected_anchor_line_no(), Some(10));
    }

    #[tokio::test]
    async fn enter_opens_selected_anchor() {
        let mut panel = OutlinePanel::new(vec![section("One", 1, 1)]);
        let messages = panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))).await.unwrap();

        assert!(matches!(messages[0], OutlinePanelMessage::OpenSelectedAnchor(1)));
    }
}
