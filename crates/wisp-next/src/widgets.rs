use crate::edit_buffer::EditBuffer;
use crate::theme::Theme;
use crate::wrap::{fit_prefix, text_position_in_wrap, wrap_text_char};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget};
use unicode_width::UnicodeWidthStr;

/// Draws the vertical scrollbar for `content_rows` of content scrolled to
/// `offset`, sized against the rows `area` can actually show.
///
/// The track length is the number of reachable scroll positions rather than the
/// row count, so the thumb reaches the bottom exactly when the last row is
/// visible.
pub fn render_vertical_scrollbar(area: Rect, buf: &mut Buffer, content_rows: usize, offset: usize) {
    let scrollable = content_rows.saturating_sub(usize::from(area.height));
    let mut state = ScrollbarState::new(scrollable).position(offset);
    StatefulWidget::render(Scrollbar::new(ScrollbarOrientation::VerticalRight), area, buf, &mut state);
}

/// A footer's key hints, each key in the accent colour ahead of its muted
/// description. Every full-screen surface labels its keys the same way.
///
/// A single space separates the pairs: the highlighted keys already break them
/// up, and these footers are long enough that anything wider wraps at 80
/// columns.
pub fn key_hints(hints: &[(&str, &str)], theme: &Theme) -> Line<'static> {
    let key_style = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(theme.muted);
    let mut spans = Vec::with_capacity(hints.len() * 3);
    for (key, description) in hints {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled((*key).to_string(), key_style));
        spans.push(Span::styled(format!(" {description}"), muted));
    }
    Line::from(spans)
}

/// Wraps `buffer`'s text to `width` columns, with the cursor's row and column
/// within the wrapped result.
///
/// For inputs embedded in scrolled content, where the terminal cursor has to be
/// placed by the caller rather than by the widget.
pub fn wrapped_with_cursor(buffer: &EditBuffer, width: usize) -> (Vec<String>, (usize, u16)) {
    (wrap_text_char(buffer.text(), width), text_position_in_wrap(&buffer.text()[..buffer.cursor()], width))
}

/// Draws `buffer` with a block cursor painted into the text.
///
/// Used where the terminal cursor is not available because the input is one row
/// of a larger rendered document rather than the focused widget.
pub fn block_cursor_spans(buffer: &EditBuffer, text_style: Style, cursor_style: Style) -> Vec<Span<'static>> {
    let text = buffer.text();
    let cursor = buffer.cursor();
    let cursor_len = text[cursor..].chars().next().map_or(0, char::len_utf8);
    // Past the end of the text there is no character to invert, so a space
    // stands in for one.
    let under_cursor = if cursor_len == 0 { " " } else { &text[cursor..cursor + cursor_len] };
    vec![
        Span::styled(text[..cursor].to_string(), text_style),
        Span::styled(under_cursor.to_string(), cursor_style),
        Span::styled(text[cursor + cursor_len..].to_string(), text_style),
    ]
}

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
