use std::borrow::Cow;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Range};

use super::line::Line;
use crossterm::style::Color;
use unicode_width::UnicodeWidthChar;

/// Returns `(row, col)` for `byte_offset` after applying the same soft-wrap
/// rules used by [`soft_wrap_line`].
pub fn soft_wrap_text_position(text: &str, offset: usize, max_width: usize) -> (usize, usize) {
    let byte_offset = clamp_to_char_boundary(text, offset);
    let max_width = Col(max_width);
    if max_width.0 == 0 {
        return (0, display_width_text(text.slice_to(byte_offset)));
    }

    let mut row_index = Row::ZERO;
    let mut row_start = ByteOffset::ZERO;

    loop {
        let (row, next_row_start) = next_text_row(text, row_start, max_width);
        if byte_offset < row.end
            || byte_offset == row.end && next_row_start != Some(row.end)
            || next_row_start.is_none()
        {
            return (row_index.0, display_width_text_until(text, row, byte_offset).0);
        }

        let Some(next_row_start) = next_row_start else {
            return (row_index.0, 0);
        };
        row_start = next_row_start;
        row_index += 1;
    }
}

pub(crate) fn soft_wrap_text_byte_offset(text: &str, target_row: usize, target_col: usize, max_width: usize) -> usize {
    let target_row = Row(target_row);
    let target_col = Col(target_col);
    let max_width = Col(max_width);
    if max_width.0 == 0 {
        return byte_offset_for_col(text, ByteOffset::ZERO..ByteOffset::end_of(text), target_col).0;
    }

    let mut row_index = Row::ZERO;
    let mut row_start = ByteOffset::ZERO;

    loop {
        let (row, next_row_start) = next_text_row(text, row_start, max_width);
        if row_index == target_row {
            return byte_offset_for_col(text, row, target_col).0;
        }

        let Some(next_row_start) = next_row_start else {
            return text.len();
        };
        row_start = next_row_start;
        row_index += 1;
    }
}

/// Truncates text to fit within `max_width` display columns, appending "..." if truncated.
/// Returns the original string borrowed when no truncation is needed.
pub fn truncate_text(text: &str, max_width: usize) -> Cow<'_, str> {
    const ELLIPSIS: &str = "...";
    const ELLIPSIS_WIDTH: usize = 3;

    if max_width == 0 {
        return Cow::Borrowed("");
    }

    let use_ellipsis = max_width >= ELLIPSIS_WIDTH;
    let budget = if use_ellipsis { max_width - ELLIPSIS_WIDTH } else { max_width };

    let mut width = 0;
    let mut fit_end = 0; // byte offset after last char fitting within budget

    for (i, ch) in text.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max_width {
            return if use_ellipsis {
                Cow::Owned(format!("{}{ELLIPSIS}", &text[..fit_end]))
            } else {
                Cow::Owned(text[..fit_end].to_owned())
            };
        }
        width += cw;
        if width <= budget {
            fit_end = i + ch.len_utf8();
        }
    }

    Cow::Borrowed(text)
}

/// Pads `text` with trailing spaces to reach `target_width` display columns.
/// Returns the original text unchanged if it already meets or exceeds the target.
pub fn pad_text_to_width(text: &str, target_width: usize) -> Cow<'_, str> {
    let current = display_width_text(text);
    if current >= target_width {
        Cow::Borrowed(text)
    } else {
        let padding = target_width - current;
        Cow::Owned(format!("{text}{}", " ".repeat(padding)))
    }
}

pub fn display_width_text(s: &str) -> usize {
    s.chars().map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0)).sum()
}

pub fn display_width_line(line: &Line) -> usize {
    line.spans().iter().map(|span| display_width_text(span.text())).sum()
}

/// Truncates a styled line to fit within `max_width` display columns.
///
/// Walks spans tracking cumulative display width, slicing at the character
/// boundary where the budget is exhausted. No ellipsis is appended — callers
/// can pad with [`Line::extend_bg_to_width`] if needed.
///
/// Row-fill metadata on the source line is preserved on the result.
pub fn truncate_line(line: &Line, max_width: usize) -> Line {
    if max_width == 0 {
        let mut empty = Line::default();
        empty.set_fill(line.fill());
        return empty;
    }

    let mut result = Line::default();
    let mut remaining = max_width;

    for span in line.spans() {
        if remaining == 0 {
            break;
        }

        let text = span.text();
        let style = span.style();
        let mut byte_end = 0;
        let mut col = 0;

        for (i, ch) in text.char_indices() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + cw > remaining {
                break;
            }
            col += cw;
            byte_end = i + ch.len_utf8();
        }

        if byte_end > 0 {
            result.push_with_style(&text[..byte_end], style);
        }
        remaining -= col;
    }

    result.set_fill(line.fill());
    result
}

pub fn soft_wrap_line(line: &Line, width: u16) -> Vec<Line> {
    if line.is_empty() {
        let mut empty = Line::new("");
        empty.set_fill(line.fill());
        return vec![empty];
    }

    let max_width = Col(width as usize);
    if max_width.0 == 0 {
        return vec![line.clone()];
    }

    let text = line.plain_text();
    let mut rows = Vec::new();
    let mut row_start = ByteOffset::ZERO;
    loop {
        let (range, next_row_start) = next_text_row(&text, row_start, max_width);
        rows.push(slice_line(line, range, line.fill()));
        let Some(next_row_start) = next_row_start else {
            return rows;
        };
        row_start = next_row_start;
    }
}

pub fn soft_wrap_lines_with_map(lines: &[Line], width: u16) -> (Vec<Line>, Vec<usize>) {
    let mut out = Vec::new();
    let mut starts = Vec::with_capacity(lines.len());

    for line in lines {
        starts.push(out.len());
        out.extend(soft_wrap_line(line, width));
    }

    (out, starts)
}

fn slice_line(line: &Line, range: Range<ByteOffset>, fill: Option<Color>) -> Line {
    let mut result = Line::default();
    let mut cursor = ByteOffset::ZERO;

    for span in line.spans() {
        let span_start = cursor;
        let span_end = cursor + span.text().len();
        cursor = span_end;

        let start = range.start.max(span_start);
        let end = range.end.min(span_end);
        if start < end {
            result.push_with_style(&span.text()[start.0 - span_start.0..end.0 - span_start.0], span.style());
        }
    }

    result.set_fill(fill);
    result
}

fn next_text_row(text: &str, row_start: ByteOffset, max_width: Col) -> (Range<ByteOffset>, Option<ByteOffset>) {
    if row_start.0 >= text.len() {
        let end = ByteOffset::end_of(text);
        return (end..end, None);
    }

    let mut row_width = Col::ZERO;
    let mut last_ws: Option<Range<ByteOffset>> = None;

    for (offset, ch) in text.slice_from(row_start).char_indices() {
        let byte_start = row_start + offset;
        let byte_end = byte_start + ch.len_utf8();

        if ch == '\n' {
            return (row_start..byte_start, Some(byte_end));
        }

        let width = col_of(ch);
        if width.0 > 0 && row_width + width > max_width && row_width.0 > 0 {
            return if ch.is_whitespace() {
                (row_start..byte_start, Some(byte_end))
            } else if let Some(ws) = last_ws {
                (row_start..ws.start, Some(ws.end))
            } else {
                (row_start..byte_start, Some(byte_start))
            };
        }

        row_width += width;
        if ch.is_whitespace() {
            last_ws = Some(byte_start..byte_end);
        }
    }

    (row_start..ByteOffset::end_of(text), None)
}

fn byte_offset_for_col(text: &str, range: Range<ByteOffset>, target_col: Col) -> ByteOffset {
    let mut col = Col::ZERO;
    let mut byte = range.start;
    for ch in text.slice(range.clone()).chars() {
        if col >= target_col {
            return byte;
        }
        col += col_of(ch);
        byte += ch.len_utf8();
        if col >= target_col {
            return byte;
        }
    }
    range.end
}

fn clamp_to_char_boundary(text: &str, byte_offset: usize) -> ByteOffset {
    let mut byte_offset = byte_offset.min(text.len());
    while !text.is_char_boundary(byte_offset) {
        byte_offset = byte_offset.saturating_sub(1);
    }
    ByteOffset(byte_offset)
}

fn display_width_text_until(text: &str, range: Range<ByteOffset>, byte_offset: ByteOffset) -> Col {
    text.slice(range.clone())
        .char_indices()
        .take_while(|(offset, ch)| range.start + *offset + ch.len_utf8() <= byte_offset)
        .map(|(_, ch)| col_of(ch))
        .sum()
}

fn col_of(ch: char) -> Col {
    Col(UnicodeWidthChar::width(ch).unwrap_or(0))
}

/// Byte offset into a UTF-8 string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ByteOffset(usize);

impl ByteOffset {
    const ZERO: Self = Self(0);
    fn end_of(text: &str) -> Self {
        Self(text.len())
    }
}

impl Add<usize> for ByteOffset {
    type Output = Self;
    fn add(self, rhs: usize) -> Self {
        Self(self.0 + rhs)
    }
}

impl AddAssign<usize> for ByteOffset {
    fn add_assign(&mut self, rhs: usize) {
        self.0 += rhs;
    }
}

/// Display column / unicode display-width count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Col(usize);

impl Col {
    const ZERO: Self = Self(0);
}

impl Add for Col {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Col {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sum for Col {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        Self(iter.map(|c| c.0).sum())
    }
}

/// Visual row index in the wrapped grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Row(usize);

impl Row {
    const ZERO: Self = Self(0);
}

impl AddAssign<usize> for Row {
    fn add_assign(&mut self, rhs: usize) {
        self.0 += rhs;
    }
}

/// Slice `&str` with [`Bytes`]-typed offsets.
trait ByteSlice {
    fn slice(&self, range: Range<ByteOffset>) -> &str;
    fn slice_from(&self, start: ByteOffset) -> &str;
    fn slice_to(&self, end: ByteOffset) -> &str;
}

impl ByteSlice for str {
    fn slice(&self, range: Range<ByteOffset>) -> &str {
        &self[range.start.0..range.end.0]
    }
    fn slice_from(&self, start: ByteOffset) -> &str {
        &self[start.0..]
    }
    fn slice_to(&self, end: ByteOffset) -> &str {
        &self[..end.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::style::Color;

    #[test]
    fn wraps_ascii_to_width() {
        let rows = soft_wrap_line(&Line::new("abcdef"), 3);
        assert_eq!(rows, vec![Line::new("abc"), Line::new("def")]);
    }

    #[test]
    fn display_width_ignores_style() {
        let mut line = Line::default();
        line.push_styled("he", Color::Red);
        line.push_text("llo");
        assert_eq!(display_width_line(&line), 5);
    }

    #[test]
    fn wraps_preserving_style_spans() {
        let line = Line::styled("abcdef", Color::Red);
        let rows = soft_wrap_line(&line, 3);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "abc");
        assert_eq!(rows[1].plain_text(), "def");
        assert_eq!(rows[0].spans().len(), 1);
        assert_eq!(rows[1].spans().len(), 1);
        assert_eq!(rows[0].spans()[0].style().fg, Some(Color::Red));
        assert_eq!(rows[1].spans()[0].style().fg, Some(Color::Red));
    }

    #[test]
    fn counts_wide_unicode() {
        assert_eq!(display_width_text("中a"), 3);
        let rows = soft_wrap_line(&Line::new("中ab"), 3);
        assert_eq!(rows, vec![Line::new("中a"), Line::new("b")]);
    }

    #[test]
    fn wraps_multi_span_line_mid_span() {
        let mut line = Line::default();
        line.push_styled("ab", Color::Red);
        line.push_styled("cd", Color::Blue);
        line.push_styled("ef", Color::Green);
        let rows = soft_wrap_line(&line, 3);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "abc");
        assert_eq!(rows[1].plain_text(), "def");
        // First row: "ab" (Red) + "c" (Blue)
        assert_eq!(rows[0].spans().len(), 2);
        assert_eq!(rows[0].spans()[0].style().fg, Some(Color::Red));
        assert_eq!(rows[0].spans()[1].style().fg, Some(Color::Blue));
        // Second row: "d" (Blue) + "ef" (Green)
        assert_eq!(rows[1].spans().len(), 2);
        assert_eq!(rows[1].spans()[0].style().fg, Some(Color::Blue));
        assert_eq!(rows[1].spans()[1].style().fg, Some(Color::Green));
    }

    #[test]
    fn wraps_line_with_embedded_newlines() {
        let line = Line::new("abc\ndef");
        let rows = soft_wrap_line(&line, 80);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "abc");
        assert_eq!(rows[1].plain_text(), "def");
    }

    #[test]
    fn pad_text_pads_ascii_to_target_width() {
        let result = pad_text_to_width("hello", 10);
        assert_eq!(result, "hello     ");
        assert_eq!(display_width_text(&result), 10);
    }

    #[test]
    fn pad_text_returns_borrowed_when_already_wide_enough() {
        let result = pad_text_to_width("hello", 5);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result, "hello");

        let result = pad_text_to_width("hello", 3);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result, "hello");
    }

    #[test]
    fn pad_text_handles_wide_unicode() {
        // "中" is 2 display columns wide
        let result = pad_text_to_width("中a", 6);
        assert_eq!(display_width_text(&result), 6);
        assert_eq!(result, "中a   "); // 2+1 = 3 cols, need 3 spaces
    }

    #[test]
    fn truncate_text_fits_within_width() {
        assert_eq!(truncate_text("hello", 10), "hello");
        assert_eq!(truncate_text("hello world", 8), "hello...");
        assert_eq!(truncate_text("hello", 5), "hello");
        assert_eq!(truncate_text("hello", 4), "h...");
    }

    #[test]
    fn truncate_text_handles_wide_unicode() {
        // Chinese characters are 2 columns wide
        assert_eq!(truncate_text("中文字", 5), "中..."); // 6 cols -> truncate to 2+3=5
        assert_eq!(truncate_text("中ab", 4), "中ab"); // 2+1+1=4, fits exactly
        assert_eq!(truncate_text("中abc", 4), "..."); // 5 cols, only ellipsis fits in 4
        assert_eq!(truncate_text("中abcde", 6), "中a..."); // 7 cols -> truncate to 2+1+3=6
    }

    #[test]
    fn truncate_text_handles_zero_width() {
        assert_eq!(truncate_text("hello", 0), "");
    }

    #[test]
    fn truncate_text_max_width_1() {
        let result = truncate_text("hello", 1);
        assert!(
            display_width_text(&result) <= 1,
            "Expected width <= 1, got '{}' (width {})",
            result,
            display_width_text(&result),
        );
        assert_eq!(result, "h");
    }

    #[test]
    fn truncate_text_max_width_2() {
        let result = truncate_text("hello", 2);
        assert!(
            display_width_text(&result) <= 2,
            "Expected width <= 2, got '{}' (width {})",
            result,
            display_width_text(&result),
        );
        assert_eq!(result, "he");
    }

    #[test]
    fn truncate_line_returns_short_lines_unchanged() {
        let line = Line::new("short");
        let result = truncate_line(&line, 20);
        assert_eq!(result.plain_text(), "short");
    }

    #[test]
    fn truncate_line_trims_long_styled_lines() {
        let mut line = Line::default();
        line.push_styled("hello", Color::Red);
        line.push_styled(" world", Color::Blue);
        let result = truncate_line(&line, 7);
        assert_eq!(result.plain_text(), "hello w");
        assert_eq!(result.spans().len(), 2);
        assert_eq!(result.spans()[0].style().fg, Some(Color::Red));
        assert_eq!(result.spans()[1].style().fg, Some(Color::Blue));
    }

    #[test]
    fn truncate_line_handles_mid_span_cut() {
        let line = Line::styled("abcdefgh", Color::Green);
        let result = truncate_line(&line, 4);
        assert_eq!(result.plain_text(), "abcd");
        assert_eq!(result.spans()[0].style().fg, Some(Color::Green));
    }

    #[test]
    fn truncate_line_handles_wide_unicode_at_boundary() {
        // "中" is 2 display columns, "文" is 2.
        // Budget of 3: "中"(2) fits, "文"(2) would exceed (2+2=4>3), so stop.
        let line = Line::new("中文x");
        let result = truncate_line(&line, 3);
        assert_eq!(result.plain_text(), "中");

        // Budget of 4: "中"(2) + "文"(2) = 4, fits exactly.
        let result = truncate_line(&line, 4);
        assert_eq!(result.plain_text(), "中文");

        // Budget of 5: all fit: 2+2+1=5.
        let result = truncate_line(&line, 5);
        assert_eq!(result.plain_text(), "中文x");
    }

    #[test]
    fn truncate_line_zero_width_returns_empty() {
        let line = Line::new("hello");
        let result = truncate_line(&line, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn soft_wrap_text_position_uses_word_boundaries() {
        assert_eq!(soft_wrap_text_position("hello world", "hello ".len(), 7), (1, 0));
        assert_eq!(soft_wrap_text_position("hello world", "hello world".len(), 7), (1, 5));
    }

    #[test]
    fn soft_wrap_text_byte_offset_uses_word_boundaries() {
        assert_eq!(soft_wrap_text_byte_offset("hello world", 0, 5, 7), 5);
        assert_eq!(soft_wrap_text_byte_offset("hello world", 1, 3, 7), 9);
    }

    #[test]
    fn soft_wrap_text_position_handles_trailing_newline() {
        assert_eq!(soft_wrap_text_position("hello\n", "hello\n".len(), 10), (1, 0));
    }

    #[test]
    fn soft_wrap_text_position_places_hard_wrap_boundary_on_next_row() {
        assert_eq!(soft_wrap_text_position("abcdef", 3, 3), (1, 0));
    }

    #[test]
    fn wraps_at_word_boundary() {
        let rows = soft_wrap_line(&Line::new("hello world"), 7);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "hello");
        assert_eq!(rows[1].plain_text(), "world");
    }

    #[test]
    fn wraps_multiple_words() {
        let rows = soft_wrap_line(&Line::new("hello world foo"), 12);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "hello world");
        assert_eq!(rows[1].plain_text(), "foo");
    }

    #[test]
    fn falls_back_to_char_break_without_whitespace() {
        let rows = soft_wrap_line(&Line::new("superlongword next"), 5);
        assert_eq!(rows[0].plain_text(), "super");
        assert_eq!(rows[1].plain_text(), "longw");
        assert_eq!(rows[2].plain_text(), "ord");
        assert_eq!(rows[3].plain_text(), "next");
    }

    #[test]
    fn wraps_at_word_boundary_with_styled_spans() {
        let line = Line::styled("hello world", Color::Red);
        let rows = soft_wrap_line(&line, 7);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "hello");
        assert_eq!(rows[1].plain_text(), "world");
        assert_eq!(rows[0].spans()[0].style().fg, Some(Color::Red));
        assert_eq!(rows[1].spans()[0].style().fg, Some(Color::Red));
    }

    #[test]
    fn wraps_at_whitespace_across_span_boundaries() {
        let mut line = Line::default();
        line.push_styled("@aaaaa", Color::Red);
        line.push_text(" ");
        line.push_styled("@bbbbbb", Color::Blue);

        let rows = soft_wrap_line(&line, 10);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "@aaaaa");
        assert_eq!(rows[1].plain_text(), "@bbbbbb");
        assert_eq!(rows[0].spans()[0].style().fg, Some(Color::Red));
        assert_eq!(rows[1].spans()[0].style().fg, Some(Color::Blue));
    }

    #[test]
    fn hard_wraps_long_styled_token_without_whitespace() {
        let line = Line::styled("@abcdefghijk", Color::Green);
        let rows = soft_wrap_line(&line, 5);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].plain_text(), "@abcd");
        assert_eq!(rows[1].plain_text(), "efghi");
        assert_eq!(rows[2].plain_text(), "jk");
        for row in &rows {
            assert_eq!(row.spans()[0].style().fg, Some(Color::Green));
        }
    }

    #[test]
    fn drops_whitespace_when_new_span_starts_at_wrap_boundary() {
        let mut line = Line::default();
        line.push_styled("abcdefghij", Color::Red);
        line.push_styled(" klm", Color::Blue);
        let rows = soft_wrap_line(&line, 10);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain_text(), "abcdefghij");
        assert_eq!(rows[1].plain_text(), "klm");
        assert_eq!(rows[1].spans()[0].style().fg, Some(Color::Blue));
    }

    #[test]
    fn soft_wrap_propagates_fill_to_each_wrapped_row() {
        let line = Line::new("abcdef").with_fill(Color::Red);
        let rows = soft_wrap_line(&line, 3);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.fill(), Some(Color::Red));
        }
    }

    #[test]
    fn soft_wrap_preserves_fill_on_empty_line() {
        let line = Line::default().with_fill(Color::Red);
        let rows = soft_wrap_line(&line, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].fill(), Some(Color::Red));
    }

    #[test]
    fn truncate_line_preserves_fill_metadata() {
        let line = Line::new("abcdef").with_fill(Color::Blue);
        let truncated = truncate_line(&line, 3);
        assert_eq!(truncated.plain_text(), "abc");
        assert_eq!(truncated.fill(), Some(Color::Blue));
    }

    #[test]
    fn wraps_across_spans_without_panic() {
        let mut line = Line::default();
        line.push_styled("hello ", Color::Red);
        line.push_styled("world this is long", Color::Blue);
        let rows = soft_wrap_line(&line, 10);
        assert_eq!(rows[0].plain_text(), "hello");
        assert_eq!(rows[1].plain_text(), "world this");
        assert_eq!(rows[2].plain_text(), "is long");
    }
}
