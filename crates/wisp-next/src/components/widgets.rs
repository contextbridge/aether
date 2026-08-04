use crate::components::edit_buffer::EditBuffer;
use crate::components::theme::Theme;
use crate::components::wrap::{as_u16, fit_prefix};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget};
use std::borrow::Cow;
use unicode_width::UnicodeWidthStr;

/// A key and what it does, for [`key_hints`]. Borrowed for the fixed labels most
/// footers use, owned for the ones that interpolate live state.
pub type KeyHint = (&'static str, Cow<'static, str>);

/// Columns a vertical scrollbar track occupies.
pub const SCROLLBAR_WIDTH: u16 = 1;

/// Splits `area` into the rows and the scrollbar track beside them, so the
/// track never sits on top of a row. Without a scrollbar the rows get it all.
///
/// Every scrolling pane in the UI — list, document, and form — carves its area
/// up this way, and builds its rows to the width this leaves.
pub fn rows_and_track(area: Rect, scrollbar: bool) -> (Rect, Rect) {
    let track_width = if scrollbar { SCROLLBAR_WIDTH } else { 0 };
    let [rows, track] = Layout::horizontal([Constraint::Min(0), Constraint::Length(track_width)]).areas(area);
    (rows, track)
}

/// The full-width, one-row `Rect` for the `index`-th visible row of `area`, or
/// nothing once `index` falls past the bottom.
pub fn row_area(area: Rect, index: usize) -> Option<Rect> {
    let offset = as_u16(index);
    (offset < area.height).then(|| Rect { y: area.y + offset, height: 1, ..area })
}

/// Draws already-built rows directly into a buffer area.
#[derive(Clone)]
pub struct RowsView<'a> {
    lines: Cow<'a, [Line<'static>]>,
}

impl<'a> RowsView<'a> {
    pub fn new(lines: &'a [Line<'static>]) -> Self {
        Self { lines: Cow::Borrowed(lines) }
    }

    pub fn from_iter(lines: impl IntoIterator<Item = Line<'static>>) -> Self {
        Self { lines: Cow::Owned(lines.into_iter().collect()) }
    }
}

impl Widget for RowsView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (index, line) in self.lines.iter().enumerate() {
            let Some(row) = row_area(area, index) else {
                return;
            };
            line.render(row, buf);
        }
    }
}

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
pub fn key_hints(hints: &[(impl AsRef<str>, impl AsRef<str>)], theme: &Theme) -> Line<'static> {
    let key_style = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(theme.muted);
    let mut spans = Vec::with_capacity(hints.len() * 3);
    for (key, description) in hints {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(key.as_ref().to_string(), key_style));
        spans.push(Span::styled(format!(" {}", description.as_ref()), muted));
    }
    Line::from(spans)
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

impl Widget for &TextInput<'_> {
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
    use crate::components::edit_buffer::EditBuffer;
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
