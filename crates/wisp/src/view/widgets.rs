use crate::theme::Theme;
use crate::view::edit_buffer::EditBuffer;
use crate::view::wrap::{as_u16, fit_prefix};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget};
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

    pub fn from_lines(lines: impl IntoIterator<Item = Line<'static>>) -> Self {
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
/// description. Every full-screen route labels its keys the same way.
///
/// A single space separates the pairs: the highlighted keys already break them
/// up, and these footers are long enough that anything wider wraps at 80
/// columns.
pub fn key_hints(hints: &[(impl AsRef<str>, impl AsRef<str>)], theme: &Theme) -> Line<'static> {
    let key_style = Style::from((theme.accent, Modifier::BOLD));
    let muted = Style::from(theme.muted);
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

pub(crate) fn visible_window(buffer: &EditBuffer, width: usize) -> (String, usize) {
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
    use super::visible_window;
    use crate::view::edit_buffer::EditBuffer;

    #[test]
    fn scrolls_unicode_text_to_keep_cursor_visible() {
        assert_eq!(visible_window(&EditBuffer::new("a界bcdef"), 4), ("def".to_string(), 3));
    }
}
