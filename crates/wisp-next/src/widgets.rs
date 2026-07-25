use crate::edit_buffer::EditBuffer;
use crate::wrap::fit_prefix;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy)]
pub struct TextInput<'a> {
    buffer: &'a EditBuffer,
    prefix: &'a str,
    style: Style,
    prefix_style: Style,
}

impl<'a> TextInput<'a> {
    pub fn new(buffer: &'a EditBuffer) -> Self {
        Self { buffer, prefix: "", style: Style::default(), prefix_style: Style::default() }
    }

    pub fn prefix(mut self, prefix: &'a str) -> Self {
        self.prefix = prefix;
        self
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn prefix_style(mut self, style: Style) -> Self {
        self.prefix_style = style;
        self
    }

    pub fn cursor_position(self, area: Rect) -> Position {
        let prefix_width = self.prefix.width().min(usize::from(area.width));
        let available = usize::from(area.width).saturating_sub(prefix_width);
        let (_, cursor_column) = visible_window(self.buffer, available);
        Position::new(area.x.saturating_add(u16::try_from(prefix_width + cursor_column).unwrap_or(u16::MAX)), area.y)
    }
}

impl Widget for TextInput<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let prefix_width = self.prefix.width().min(usize::from(area.width));
        let available = usize::from(area.width).saturating_sub(prefix_width);
        let (visible, _) = visible_window(self.buffer, available);
        Paragraph::new(Line::from(vec![
            Span::styled(self.prefix, self.prefix_style),
            Span::styled(visible, self.style),
        ]))
        .render(area, buffer);
    }
}

fn visible_window(buffer: &EditBuffer, width: usize) -> (String, usize) {
    if width == 0 {
        return (String::new(), 0);
    }
    let text = buffer.text();
    let cursor_column = text[..buffer.cursor()].width();
    let start_column = cursor_column.saturating_sub(width.saturating_sub(1));
    let (start, _) = fit_prefix(text, start_column);
    let cursor_width = text[start..buffer.cursor()].width();
    let (visible_len, _) = fit_prefix(&text[start..], width);
    (text[start..start + visible_len].to_string(), cursor_width.min(width.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::TextInput;
    use crate::edit_buffer::EditBuffer;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    #[test]
    fn scrolls_unicode_text_to_keep_cursor_visible() {
        let input = EditBuffer::new("a界bcdef");
        let widget = TextInput::new(&input).prefix("> ");
        assert_eq!(widget.cursor_position(Rect::new(0, 0, 6, 1)).x, 5);

        let mut terminal = Terminal::new(TestBackend::new(6, 1)).unwrap();
        terminal.draw(|frame| frame.render_widget(widget, frame.area())).unwrap();
        let rendered =
            terminal.backend().buffer().content.iter().map(ratatui::buffer::Cell::symbol).collect::<String>();
        assert!(rendered.contains("def"));
    }
}
