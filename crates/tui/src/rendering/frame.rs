use super::line::Line;
use super::soft_wrap::{soft_wrap_lines_with_map, truncate_line};

/// Logical cursor position within a component's rendered output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub is_visible: bool,
}

impl Cursor {
    /// Create a hidden cursor at position (0, 0).
    pub fn hidden() -> Self {
        Self::default()
    }

    /// Create a visible cursor at the given position.
    pub fn visible(row: usize, col: usize) -> Self {
        Self { row, col, is_visible: true }
    }

    /// Shift a visible cursor right by `delta` columns. Hidden cursors are
    /// returned unchanged.
    pub fn shift_col(self, delta: usize) -> Self {
        if self.is_visible { Self { col: self.col + delta, ..self } } else { self }
    }
}

/// Overflow policy used by [`Frame::fit`] when content exceeds the target width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// Wrap rows that exceed the width onto additional visual rows.
    Wrap,
    /// Truncate rows that exceed the width. Row count is preserved.
    Truncate,
}

/// Configuration for [`Frame::fit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FitOptions {
    pub overflow_x: Overflow,
    /// When true, every resulting row is padded to `width` so the row visually
    /// fills its allocated box. Padding inherits any background color present
    /// on the row.
    pub fill_x: bool,
}

impl FitOptions {
    /// Wrapping fit, no fill.
    pub fn wrap() -> Self {
        Self { overflow_x: Overflow::Wrap, fill_x: false }
    }

    /// Truncating fit, no fill.
    pub fn truncate() -> Self {
        Self { overflow_x: Overflow::Truncate, fill_x: false }
    }

    /// Builder: enable row fill.
    pub fn with_fill(mut self) -> Self {
        self.fill_x = true;
        self
    }
}

/// A horizontally-stacked slot in [`Frame::hstack`].
///
/// Each part holds a child frame and the width it occupies in the composed
/// output. Use [`FramePart::fit`] / [`FramePart::wrap`] / [`FramePart::truncate`]
/// to construct a slot from an unrestricted child frame — they fit the inner
/// frame to `width` for you. Use [`FramePart::new`] only when the caller has
/// already guaranteed the frame fits the slot (e.g., a separator column built
/// at exactly the right width).
#[derive(Debug, Clone)]
pub struct FramePart {
    pub frame: Frame,
    pub width: u16,
}

impl FramePart {
    /// Construct a slot from an already-fitted frame. Caller is responsible
    /// for ensuring `frame` does not exceed `width`. Prefer `fit` / `wrap` /
    /// `truncate` for unrestricted children.
    pub fn new(frame: Frame, width: u16) -> Self {
        Self { frame, width }
    }

    /// Fit `frame` to `width` with the given options before adopting it as a
    /// slot. This is the right constructor for slots built from arbitrary
    /// child output.
    pub fn fit(frame: Frame, width: u16, options: FitOptions) -> Self {
        Self { frame: frame.fit(width, options), width }
    }

    /// Shorthand for `FramePart::fit(frame, width, FitOptions::wrap().with_fill())`.
    /// Soft-wraps the child to the slot width and marks each row to fill its
    /// background, so the slot paints to the right edge.
    pub fn wrap(frame: Frame, width: u16) -> Self {
        Self::fit(frame, width, FitOptions::wrap().with_fill())
    }

    /// Shorthand for `FramePart::fit(frame, width, FitOptions::truncate().with_fill())`.
    /// Truncates each row of the child to the slot width and marks each row
    /// to fill its background.
    pub fn truncate(frame: Frame, width: u16) -> Self {
        Self::fit(frame, width, FitOptions::truncate().with_fill())
    }
}

#[doc = include_str!("../docs/frame.md")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    lines: Vec<Line>,
    cursor: Cursor,
}

impl Frame {
    pub fn new(lines: Vec<Line>) -> Self {
        Self { lines, cursor: Cursor::hidden() }
    }

    /// An empty frame with a hidden cursor.
    pub fn empty() -> Self {
        Self { lines: Vec::new(), cursor: Cursor::hidden() }
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Replace the cursor without cloning lines.
    pub fn with_cursor(mut self, cursor: Cursor) -> Self {
        self.cursor = cursor;
        self
    }

    pub fn into_lines(self) -> Vec<Line> {
        self.lines
    }

    pub fn into_parts(self) -> (Vec<Line>, Cursor) {
        (self.lines, self.cursor)
    }

    pub fn clamp_cursor(mut self) -> Self {
        if self.cursor.row >= self.lines.len() {
            self.cursor.row = self.lines.len().saturating_sub(1);
        }
        self
    }

    /// Fit the frame to a target width.
    ///
    /// - [`Overflow::Wrap`]: each line is soft-wrapped to `width`. The cursor
    ///   row is remapped to the wrapped visual row, and the cursor column is
    ///   reduced modulo `width`, with the row advanced for any overflow. This
    ///   matches the wrap math used by `VisualFrame`.
    /// - [`Overflow::Truncate`]: each line is truncated to `width`. Row count
    ///   is unchanged. The cursor column is clamped to `width.saturating_sub(1)`.
    /// - `fill_x`: every resulting row is marked with row-fill metadata using
    ///   any background color present on the row. The fill is **not**
    ///   materialized into trailing spaces here — that happens later, in
    ///   [`Frame::hstack`] (per slot width) or
    ///   `VisualFrame` (per terminal width).
    ///   Deferring materialization is what prevents trailing-space rows from
    ///   producing phantom rows when wrapped again at a smaller width.
    ///
    /// As a special case, `width == 0` returns the frame unchanged with a
    /// hidden cursor (matching the zero-width behavior of `VisualFrame`).
    pub fn fit(self, width: u16, options: FitOptions) -> Self {
        if width == 0 {
            return Self { lines: self.lines, cursor: Cursor::hidden() };
        }

        match options.overflow_x {
            Overflow::Wrap => self.fit_wrap(width, options.fill_x),
            Overflow::Truncate => self.fit_truncate(width, options.fill_x),
        }
    }

    /// Shift visual content `cols` columns to the right by prepending spaces
    /// to each line. The cursor column is shifted by `cols`. The row is
    /// unchanged.
    ///
    /// The prepended prefix inherits any background color from the line, so
    /// row-fill highlights extend through the indent.
    pub fn indent(self, cols: u16) -> Self {
        if cols == 0 {
            return self;
        }
        let prefix = " ".repeat(usize::from(cols));
        let lines = self.lines.into_iter().map(|line| line.prepend(prefix.clone())).collect();
        Self { lines, cursor: self.cursor.shift_col(usize::from(cols)) }
    }

    /// Concatenate frames vertically.
    ///
    /// The first frame in the iterator that has a visible cursor wins. Its
    /// row is offset by the cumulative line count of all preceding frames.
    pub fn vstack(frames: impl IntoIterator<Item = Frame>) -> Self {
        let mut all_lines: Vec<Line> = Vec::new();
        let mut cursor = Cursor::hidden();
        for frame in frames {
            let row_offset = all_lines.len();
            if !cursor.is_visible && frame.cursor.is_visible {
                cursor = Cursor { row: frame.cursor.row + row_offset, col: frame.cursor.col, is_visible: true };
            }
            all_lines.extend(frame.lines);
        }
        Self { lines: all_lines, cursor }
    }

    /// Compose frames horizontally into fixed-width slots.
    ///
    /// Each part's frame is assumed to already fit its `width` (callers should
    /// `fit(width, FitOptions::wrap())` or similar first). Any row-fill
    /// metadata on a part's rows is materialized to the part's slot width
    /// before merging, so trailing fill never bleeds into a neighboring slot.
    /// Heights are balanced by padding shorter frames with blank rows of the
    /// slot's width. The first part with a visible cursor wins; its column is
    /// offset by the cumulative width of preceding slots.
    pub fn hstack(parts: impl IntoIterator<Item = FramePart>) -> Self {
        let parts: Vec<FramePart> = parts.into_iter().collect();
        if parts.is_empty() {
            return Self::empty();
        }

        let max_rows = parts.iter().map(|p| p.frame.lines.len()).max().unwrap_or(0);

        let mut cursor = Cursor::hidden();
        let mut col_offset: usize = 0;
        for part in &parts {
            if !cursor.is_visible && part.frame.cursor.is_visible {
                cursor =
                    Cursor { row: part.frame.cursor.row, col: part.frame.cursor.col + col_offset, is_visible: true };
            }
            col_offset += usize::from(part.width);
        }

        let mut merged: Vec<Line> = Vec::with_capacity(max_rows);
        for row_idx in 0..max_rows {
            let mut row = Line::default();
            for part in &parts {
                let slot_width = usize::from(part.width);
                let Some(line) = part.frame.lines.get(row_idx) else {
                    row.push_text(" ".repeat(slot_width));
                    continue;
                };
                if line.fill().is_some() {
                    let mut materialized = line.clone();
                    materialized.extend_bg_to_width(slot_width);
                    row.append_line(&materialized);
                } else {
                    row.append_line(line);
                    let line_width = line.display_width();
                    if line_width < slot_width {
                        row.push_text(" ".repeat(slot_width - line_width));
                    }
                }
            }
            merged.push(row);
        }

        Self { lines: merged, cursor }
    }

    /// Apply `f` to each line in turn, preserving cursor and overall row
    /// count. The function may not split or merge rows; doing so will leave
    /// the cursor pointing at the wrong row.
    pub fn map_lines<T: FnMut(Line) -> Line>(self, f: T) -> Self {
        let lines = self.lines.into_iter().map(f).collect();
        Self { lines, cursor: self.cursor }
    }

    /// Prepend a fixed-width gutter to each row. The first row gets `head`,
    /// subsequent rows get `tail`. Use the same value for both for a uniform
    /// gutter, or different values for first/continuation patterns (e.g.,
    /// line numbers on the first row of a wrapped block, blanks on the rest).
    ///
    /// `head` and `tail` must have equal display width — debug-asserted. The
    /// cursor column is shifted by that width. Any row-fill metadata on the
    /// original row is preserved on the prefixed row.
    pub fn prefix(self, head: &Line, tail: &Line) -> Self {
        let shift = head.display_width();
        debug_assert_eq!(shift, tail.display_width(), "Frame::prefix: head and tail must have equal display width");
        let lines: Vec<Line> = self
            .lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                let prefix_src = if i == 0 { head } else { tail };
                let row_fill = line.fill();
                let mut prefixed = Line::default();
                prefixed.append_line(prefix_src);
                prefixed.append_line(&line);
                prefixed.set_fill(row_fill);
                prefixed
            })
            .collect();

        Self { lines, cursor: self.cursor.shift_col(shift) }
    }

    /// Pad with blank rows of `width` columns until at least `target` rows
    /// total. No-op if already at or above `target`. Cursor preserved.
    pub fn pad_height(self, target: u16, width: u16) -> Self {
        let target_usize = usize::from(target);
        let mut lines = self.lines;
        if lines.len() < target_usize {
            let blank = Line::new(" ".repeat(usize::from(width)));
            lines.resize(target_usize, blank);
        }
        Self { lines, cursor: self.cursor }
    }

    /// Truncate to at most `target` rows. If the visible cursor falls beyond
    /// the truncation, it is hidden.
    pub fn truncate_height(self, target: u16) -> Self {
        let target_usize = usize::from(target);
        let mut lines = self.lines;
        if lines.len() > target_usize {
            lines.truncate(target_usize);
        }
        let cursor =
            if self.cursor.is_visible && self.cursor.row >= target_usize { Cursor::hidden() } else { self.cursor };
        Self { lines, cursor }
    }

    /// Force the frame to exactly `target` rows: truncate if taller, pad with
    /// blank rows of `width` columns if shorter. Convenience for layouts that
    /// emit a fixed-height region regardless of child content.
    pub fn fit_height(self, target: u16, width: u16) -> Self {
        self.truncate_height(target).pad_height(target, width)
    }

    /// Wrap each row in side chrome: materialize fill to `inner_width`, then
    /// prepend `left` and append `right` to every row. The cursor column is
    /// shifted by `left.display_width()`.
    ///
    /// Used for borders/box chrome where the row's interior should fill its
    /// allocated width before the right edge is appended.
    pub fn wrap_each(self, inner_width: u16, left: &Line, right: &Line) -> Self {
        let inner_width_usize = usize::from(inner_width);
        let left_width = left.display_width();
        let lines: Vec<Line> = self
            .lines
            .into_iter()
            .map(|mut line| {
                line.extend_bg_to_width(inner_width_usize);
                let mut wrapped = Line::default();
                wrapped.append_line(left);
                wrapped.append_line(&line);
                wrapped.append_line(right);
                wrapped
            })
            .collect();
        Self { lines, cursor: self.cursor.shift_col(left_width) }
    }

    /// Insert the lines of `other` into this frame immediately after `after_row`.
    ///
    /// Rows `0..=after_row` remain in place. The lines of `other` appear
    /// between row `after_row` and what was previously row `after_row + 1`.
    /// Rows that followed the insertion point shift down by
    /// `other.lines().len()`.
    ///
    /// If `after_row >= self.lines.len()`, the lines are appended at the end.
    ///
    /// Cursor rule (consistent with `vstack` / `hstack`): the host frame's
    /// visible cursor takes priority — shifted down when it sits after the
    /// insertion point. Otherwise, `other`'s visible cursor is adopted with
    /// its row offset to the insertion position.
    pub fn splice(self, after_row: usize, other: Frame) -> Self {
        let inserted_count = other.lines.len();
        if inserted_count == 0 {
            return self;
        }

        let split_at = (after_row + 1).min(self.lines.len());
        let mut lines = self.lines;
        let tail: Vec<Line> = lines.drain(split_at..).collect();
        lines.extend(other.lines);
        lines.extend(tail);

        let cursor = if self.cursor.is_visible {
            if self.cursor.row > after_row {
                Cursor { row: self.cursor.row + inserted_count, ..self.cursor }
            } else {
                self.cursor
            }
        } else if other.cursor.is_visible {
            Cursor { row: other.cursor.row + split_at, col: other.cursor.col, is_visible: true }
        } else {
            Cursor::hidden()
        };

        Self { lines, cursor }
    }

    /// Drop the first `offset` rows and keep at most `height` rows.
    ///
    /// The cursor row is shifted up by `offset`. If the cursor falls outside
    /// the visible window it is hidden.
    pub fn scroll(self, offset: usize, height: usize) -> Self {
        let end = (offset + height).min(self.lines.len());
        let visible: Vec<Line> = self.lines.into_iter().skip(offset).take(height).collect();

        let cursor = if self.cursor.is_visible && self.cursor.row >= offset && self.cursor.row < end {
            Cursor { row: self.cursor.row - offset, col: self.cursor.col, is_visible: true }
        } else {
            Cursor::hidden()
        };

        Self { lines: visible, cursor }
    }

    fn fit_wrap(self, width: u16, fill_x: bool) -> Self {
        let (mut wrapped_lines, logical_to_visual) = soft_wrap_lines_with_map(&self.lines, width);

        let cursor = if self.cursor.is_visible {
            let mut visual_row = logical_to_visual
                .get(self.cursor.row)
                .copied()
                .unwrap_or_else(|| wrapped_lines.len().saturating_sub(1));
            let mut visual_col = self.cursor.col;
            let width_usize = usize::from(width);
            visual_row += visual_col / width_usize;
            visual_col %= width_usize;
            if visual_row >= wrapped_lines.len() {
                visual_row = wrapped_lines.len().saturating_sub(1);
            }
            Cursor { row: visual_row, col: visual_col, is_visible: true }
        } else {
            Cursor::hidden()
        };

        apply_fill_metadata(&mut wrapped_lines, fill_x);
        Self { lines: wrapped_lines, cursor }
    }

    fn fit_truncate(self, width: u16, fill_x: bool) -> Self {
        let width_usize = usize::from(width);
        let mut lines: Vec<Line> = self.lines.iter().map(|line| truncate_line(line, width_usize)).collect();

        apply_fill_metadata(&mut lines, fill_x);

        let cursor = if self.cursor.is_visible {
            let max_col = width_usize.saturating_sub(1);
            Cursor { row: self.cursor.row, col: self.cursor.col.min(max_col), is_visible: true }
        } else {
            Cursor::hidden()
        };

        Self { lines, cursor }
    }
}

fn apply_fill_metadata(lines: &mut [Line], fill_x: bool) {
    if !fill_x {
        return;
    }
    for line in lines {
        line.set_fill(line.infer_fill_color());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_hidden_returns_invisible_cursor_at_origin() {
        let cursor = Cursor::hidden();
        assert_eq!(cursor.row, 0);
        assert_eq!(cursor.col, 0);
        assert!(!cursor.is_visible);
    }

    #[test]
    fn cursor_visible_returns_visible_cursor_at_position() {
        let cursor = Cursor::visible(5, 10);
        assert_eq!(cursor.row, 5);
        assert_eq!(cursor.col, 10);
        assert!(cursor.is_visible);
    }

    #[test]
    fn clamp_cursor_clamps_out_of_bounds_row() {
        let frame = Frame::new(vec![Line::new("a")]).with_cursor(Cursor::visible(10, 100));
        let frame = frame.clamp_cursor();
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 100);
    }

    #[test]
    fn with_cursor_replaces_cursor_without_cloning_lines() {
        let frame = Frame::new(vec![Line::new("hello")]);
        let new_cursor = Cursor::visible(0, 3);
        let frame = frame.with_cursor(new_cursor);
        assert_eq!(frame.cursor(), new_cursor);
        assert_eq!(frame.lines()[0].plain_text(), "hello");
    }

    #[test]
    fn fit_wrap_breaks_long_line_into_multiple_rows() {
        let frame = Frame::new(vec![Line::new("abcdef")]);
        let frame = frame.fit(3, FitOptions::wrap());
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[0].plain_text(), "abc");
        assert_eq!(frame.lines()[1].plain_text(), "def");
    }

    #[test]
    fn fit_wrap_remaps_cursor_on_wrapped_row() {
        let frame = Frame::new(vec![Line::new("abcdef")]).with_cursor(Cursor::visible(0, 5));
        let frame = frame.fit(3, FitOptions::wrap());
        // col 5 → row += 5/3 = 1, col = 5%3 = 2
        assert_eq!(frame.cursor().row, 1);
        assert_eq!(frame.cursor().col, 2);
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn fit_wrap_remaps_cursor_across_logical_rows() {
        let frame = Frame::new(vec![Line::new("abcdef"), Line::new("xy")]).with_cursor(Cursor::visible(1, 1));
        let frame = frame.fit(3, FitOptions::wrap());
        // logical row 0 wraps to 2 visual rows ("abc","def"), logical row 1 starts at visual row 2.
        // Cursor at logical (1, 1) → visual (2, 1).
        assert_eq!(frame.cursor().row, 2);
        assert_eq!(frame.cursor().col, 1);
    }

    #[test]
    fn fit_wrap_hides_cursor_when_logical_row_is_invisible() {
        let frame = Frame::new(vec![Line::new("abcdef")]);
        let frame = frame.fit(3, FitOptions::wrap());
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn fit_wrap_with_fill_marks_each_row_with_fill_metadata_only() {
        use crate::style::Style;
        use crossterm::style::Color;
        // fit(...with_fill()) defers materialization. Each wrapped row has no
        // trailing spaces yet — the fill metadata is set so that hstack /
        // VisualFrame can materialize against the appropriate target width.
        let frame = Frame::new(vec![Line::with_style("abcdef", Style::default().bg_color(Color::Blue))]);
        let frame = frame.fit(4, FitOptions::wrap().with_fill());
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[0].plain_text(), "abcd");
        assert_eq!(frame.lines()[1].plain_text(), "ef");
        for line in frame.lines() {
            assert_eq!(line.fill(), Some(Color::Blue), "fill metadata should be set");
        }
    }

    #[test]
    fn fit_wrap_zero_width_returns_lines_unchanged_and_hides_cursor() {
        let frame = Frame::new(vec![Line::new("abc")]).with_cursor(Cursor::visible(0, 1));
        let frame = frame.fit(0, FitOptions::wrap());
        assert_eq!(frame.lines().len(), 1);
        assert_eq!(frame.lines()[0].plain_text(), "abc");
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn fit_truncate_cuts_each_row_to_width() {
        let frame = Frame::new(vec![Line::new("abcdef"), Line::new("xyz")]);
        let frame = frame.fit(3, FitOptions::truncate());
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[0].plain_text(), "abc");
        assert_eq!(frame.lines()[1].plain_text(), "xyz");
    }

    #[test]
    fn fit_truncate_clamps_cursor_col_within_width() {
        let frame = Frame::new(vec![Line::new("abcdef")]).with_cursor(Cursor::visible(0, 10));
        let frame = frame.fit(3, FitOptions::truncate());
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 2); // width - 1
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn fit_truncate_preserves_in_range_cursor() {
        let frame = Frame::new(vec![Line::new("abcdef")]).with_cursor(Cursor::visible(0, 1));
        let frame = frame.fit(5, FitOptions::truncate());
        assert_eq!(frame.cursor().col, 1);
    }

    #[test]
    fn fit_truncate_preserves_row_count() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]);
        let frame = frame.fit(2, FitOptions::truncate());
        assert_eq!(frame.lines().len(), 3);
    }

    #[test]
    fn fit_truncate_with_fill_marks_rows_with_fill_metadata_only() {
        use crate::style::Style;
        use crossterm::style::Color;
        let frame = Frame::new(vec![Line::with_style("ab", Style::default().bg_color(Color::Red))]);
        let frame = frame.fit(5, FitOptions::truncate().with_fill());
        // No materialization here — content is unchanged but fill is set.
        assert_eq!(frame.lines()[0].plain_text(), "ab");
        assert_eq!(frame.lines()[0].fill(), Some(Color::Red));
    }

    #[test]
    fn indent_prepends_spaces_to_each_line() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b")]);
        let frame = frame.indent(2);
        assert_eq!(frame.lines()[0].plain_text(), "  a");
        assert_eq!(frame.lines()[1].plain_text(), "  b");
    }

    #[test]
    fn indent_shifts_cursor_col() {
        let frame = Frame::new(vec![Line::new("hi")]).with_cursor(Cursor::visible(0, 1));
        let frame = frame.indent(3);
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 4);
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn indent_zero_is_noop() {
        let frame = Frame::new(vec![Line::new("hi")]).with_cursor(Cursor::visible(0, 1));
        let original_text = frame.lines()[0].plain_text();
        let original_cursor = frame.cursor();
        let frame = frame.indent(0);
        assert_eq!(frame.lines()[0].plain_text(), original_text);
        assert_eq!(frame.cursor(), original_cursor);
    }

    #[test]
    fn indent_carries_background_through_prefix() {
        use crate::style::Style;
        use crossterm::style::Color;
        let frame = Frame::new(vec![Line::with_style("hi", Style::default().bg_color(Color::Blue))]);
        let frame = frame.indent(2);
        let line = &frame.lines()[0];
        // Prefix span should carry the background color from the original line.
        assert_eq!(line.spans()[0].style().bg, Some(Color::Blue));
        assert_eq!(line.plain_text(), "  hi");
    }

    #[test]
    fn indent_does_not_make_hidden_cursor_visible() {
        let frame = Frame::new(vec![Line::new("hi")]);
        let frame = frame.indent(2);
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn vstack_empty_input_produces_empty_frame() {
        let frame = Frame::vstack(std::iter::empty());
        assert!(frame.lines().is_empty());
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn vstack_concatenates_in_order() {
        let a = Frame::new(vec![Line::new("a1"), Line::new("a2")]);
        let b = Frame::new(vec![Line::new("b1")]);
        let frame = Frame::vstack([a, b]);
        assert_eq!(frame.lines().len(), 3);
        assert_eq!(frame.lines()[0].plain_text(), "a1");
        assert_eq!(frame.lines()[1].plain_text(), "a2");
        assert_eq!(frame.lines()[2].plain_text(), "b1");
    }

    #[test]
    fn vstack_offsets_cursor_by_preceding_line_count() {
        let a = Frame::new(vec![Line::new("a1"), Line::new("a2")]);
        let b = Frame::new(vec![Line::new("b1")]).with_cursor(Cursor::visible(0, 0));
        let frame = Frame::vstack([a, b]);
        assert_eq!(frame.cursor().row, 2);
        assert_eq!(frame.cursor().col, 0);
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn vstack_first_visible_cursor_wins() {
        let a = Frame::new(vec![Line::new("a")]).with_cursor(Cursor::visible(0, 1));
        let b = Frame::new(vec![Line::new("b")]).with_cursor(Cursor::visible(0, 5));
        let frame = Frame::vstack([a, b]);
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 1);
    }

    #[test]
    fn vstack_no_visible_cursor_returns_hidden_cursor() {
        let a = Frame::new(vec![Line::new("a")]);
        let b = Frame::new(vec![Line::new("b")]);
        let frame = Frame::vstack([a, b]);
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn hstack_empty_input_produces_empty_frame() {
        let frame = Frame::hstack(std::iter::empty());
        assert!(frame.lines().is_empty());
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn hstack_merges_equal_height_parts_row_by_row() {
        let left = Frame::new(vec![Line::new("aa"), Line::new("bb")]);
        let right = Frame::new(vec![Line::new("XX"), Line::new("YY")]);
        let frame = Frame::hstack([FramePart::new(left, 2), FramePart::new(right, 2)]);
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[0].plain_text(), "aaXX");
        assert_eq!(frame.lines()[1].plain_text(), "bbYY");
    }

    #[test]
    fn hstack_pads_shorter_part_with_blank_rows() {
        let left = Frame::new(vec![Line::new("aa"), Line::new("bb")]);
        let right = Frame::new(vec![Line::new("XX")]);
        let frame = Frame::hstack([FramePart::new(left, 2), FramePart::new(right, 2)]);
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[0].plain_text(), "aaXX");
        assert_eq!(frame.lines()[1].plain_text(), "bb  ");
    }

    #[test]
    fn hstack_left_visible_cursor_unchanged_col() {
        let left = Frame::new(vec![Line::new("aa")]).with_cursor(Cursor::visible(0, 1));
        let right = Frame::new(vec![Line::new("XX")]);
        let frame = Frame::hstack([FramePart::new(left, 2), FramePart::new(right, 2)]);
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 1);
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn hstack_right_visible_cursor_offset_by_left_width() {
        let left = Frame::new(vec![Line::new("aaa")]);
        let right = Frame::new(vec![Line::new("XX")]).with_cursor(Cursor::visible(0, 1));
        let frame = Frame::hstack([FramePart::new(left, 3), FramePart::new(right, 2)]);
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 1 + 3);
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn hstack_first_visible_cursor_wins_when_both_present() {
        let left = Frame::new(vec![Line::new("aa")]).with_cursor(Cursor::visible(0, 0));
        let right = Frame::new(vec![Line::new("XX")]).with_cursor(Cursor::visible(0, 1));
        let frame = Frame::hstack([FramePart::new(left, 2), FramePart::new(right, 2)]);
        assert_eq!(frame.cursor().col, 0);
    }

    #[test]
    fn hstack_no_visible_cursor_returns_hidden_cursor() {
        let left = Frame::new(vec![Line::new("aa")]);
        let right = Frame::new(vec![Line::new("XX")]);
        let frame = Frame::hstack([FramePart::new(left, 2), FramePart::new(right, 2)]);
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn hstack_materializes_fill_to_each_part_slot_width() {
        use crate::style::Style;
        use crossterm::style::Color;

        let left =
            Frame::new(vec![Line::with_style("hi", Style::default().bg_color(Color::Red)).with_fill(Color::Red)]);
        let right = Frame::new(vec![Line::new("XX")]);
        let frame = Frame::hstack([FramePart::new(left, 5), FramePart::new(right, 2)]);
        // Left slot should be expanded to width 5 with red fill, then "XX" appended.
        assert_eq!(frame.lines()[0].plain_text(), "hi   XX");
        // Materialized; no fill metadata leaks through.
        assert_eq!(frame.lines()[0].fill(), None);
    }

    #[test]
    fn fit_wrap_with_fill_propagates_metadata_to_wrapped_rows() {
        use crate::style::Style;
        use crossterm::style::Color;

        let line = Line::with_style("abcdefgh", Style::default().bg_color(Color::Blue));
        let frame = Frame::new(vec![line]).fit(3, FitOptions::wrap().with_fill());
        assert_eq!(frame.lines().len(), 3);
        for row in frame.lines() {
            assert_eq!(row.fill(), Some(Color::Blue), "every wrapped row should carry fill metadata");
        }
    }

    #[test]
    fn hstack_three_parts_offsets_cursor_by_cumulative_widths() {
        let left = Frame::new(vec![Line::new("aa")]);
        let mid = Frame::new(vec![Line::new("|")]);
        let right = Frame::new(vec![Line::new("XX")]).with_cursor(Cursor::visible(0, 1));
        let frame = Frame::hstack([FramePart::new(left, 2), FramePart::new(mid, 1), FramePart::new(right, 2)]);
        assert_eq!(frame.lines()[0].plain_text(), "aa|XX");
        assert_eq!(frame.cursor().col, 1 + 2 + 1);
    }

    #[test]
    fn map_lines_applies_function_to_each_line() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b")]);
        let frame = frame.map_lines(|mut line| {
            line.push_text("!");
            line
        });
        assert_eq!(frame.lines()[0].plain_text(), "a!");
        assert_eq!(frame.lines()[1].plain_text(), "b!");
    }

    #[test]
    fn map_lines_preserves_cursor() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b")]).with_cursor(Cursor::visible(1, 0));
        let frame = frame.map_lines(|line| line);
        assert_eq!(frame.cursor(), Cursor::visible(1, 0));
    }

    #[test]
    fn map_lines_preserves_row_count() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]);
        let frame = frame.map_lines(|line| line);
        assert_eq!(frame.lines().len(), 3);
    }

    #[test]
    fn prefix_uses_head_on_first_row_and_tail_on_rest() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]);
        let frame = frame.prefix(&Line::new("> "), &Line::new("  "));
        assert_eq!(frame.lines()[0].plain_text(), "> a");
        assert_eq!(frame.lines()[1].plain_text(), "  b");
        assert_eq!(frame.lines()[2].plain_text(), "  c");
    }

    #[test]
    fn prefix_shifts_cursor_col_by_gutter_width() {
        let frame = Frame::new(vec![Line::new("hi")]).with_cursor(Cursor::visible(0, 1));
        let frame = frame.prefix(&Line::new("> "), &Line::new("  "));
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 1 + 2);
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn prefix_does_not_make_hidden_cursor_visible() {
        let frame = Frame::new(vec![Line::new("a")]);
        let frame = frame.prefix(&Line::new("> "), &Line::new("  "));
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn prefix_preserves_row_fill_metadata() {
        use crossterm::style::Color;
        let line = Line::new("hi").with_fill(Color::Blue);
        let frame = Frame::new(vec![line]);
        let frame = frame.prefix(&Line::new("> "), &Line::new("  "));
        assert_eq!(frame.lines()[0].fill(), Some(Color::Blue), "row-fill metadata should pass through prefix");
    }

    #[test]
    fn prefix_carries_styled_head_into_output() {
        use crate::style::Style;
        use crossterm::style::Color;
        let head = Line::with_style("├─ ", Style::fg(Color::Yellow));
        let tail = Line::with_style("   ", Style::fg(Color::Yellow));
        let frame = Frame::new(vec![Line::new("a"), Line::new("b")]).prefix(&head, &tail);
        assert_eq!(frame.lines()[0].spans()[0].style().fg, Some(Color::Yellow));
        assert_eq!(frame.lines()[1].spans()[0].style().fg, Some(Color::Yellow));
    }

    #[test]
    fn prefix_empty_frame_returns_empty() {
        let frame = Frame::empty().prefix(&Line::new("> "), &Line::new("  "));
        assert!(frame.lines().is_empty());
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn pad_height_appends_blank_rows_to_reach_target() {
        let frame = Frame::new(vec![Line::new("a")]);
        let frame = frame.pad_height(3, 4);
        assert_eq!(frame.lines().len(), 3);
        assert_eq!(frame.lines()[0].plain_text(), "a");
        assert_eq!(frame.lines()[1].plain_text(), "    ");
        assert_eq!(frame.lines()[2].plain_text(), "    ");
    }

    #[test]
    fn pad_height_no_op_if_already_at_or_above_target() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]);
        let frame = frame.pad_height(2, 4);
        assert_eq!(frame.lines().len(), 3);
    }

    #[test]
    fn pad_height_preserves_cursor() {
        let frame = Frame::new(vec![Line::new("a")]).with_cursor(Cursor::visible(0, 1));
        let frame = frame.pad_height(3, 4);
        assert_eq!(frame.cursor(), Cursor::visible(0, 1));
    }

    #[test]
    fn truncate_height_drops_excess_rows() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c"), Line::new("d")]);
        let frame = frame.truncate_height(2);
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[0].plain_text(), "a");
        assert_eq!(frame.lines()[1].plain_text(), "b");
    }

    #[test]
    fn truncate_height_hides_cursor_when_row_falls_outside() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]).with_cursor(Cursor::visible(2, 0));
        let frame = frame.truncate_height(2);
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn truncate_height_preserves_cursor_when_in_range() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]).with_cursor(Cursor::visible(1, 0));
        let frame = frame.truncate_height(2);
        assert_eq!(frame.cursor(), Cursor::visible(1, 0));
    }

    #[test]
    fn truncate_height_no_op_if_already_below_target() {
        let frame = Frame::new(vec![Line::new("a")]);
        let frame = frame.truncate_height(5);
        assert_eq!(frame.lines().len(), 1);
    }

    #[test]
    fn fit_height_truncates_taller_frames() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c"), Line::new("d")]);
        let frame = frame.fit_height(2, 4);
        assert_eq!(frame.lines().len(), 2);
    }

    #[test]
    fn fit_height_pads_shorter_frames() {
        let frame = Frame::new(vec![Line::new("a")]);
        let frame = frame.fit_height(3, 4);
        assert_eq!(frame.lines().len(), 3);
        assert_eq!(frame.lines()[1].plain_text(), "    ");
    }

    #[test]
    fn wrap_each_adds_left_and_right_to_each_row() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("bb")]);
        let frame = frame.wrap_each(3, &Line::new("│ "), &Line::new(" │"));
        assert_eq!(frame.lines()[0].plain_text(), "│ a   │");
        assert_eq!(frame.lines()[1].plain_text(), "│ bb  │");
    }

    #[test]
    fn wrap_each_shifts_cursor_col_by_left_width() {
        let frame = Frame::new(vec![Line::new("hi")]).with_cursor(Cursor::visible(0, 1));
        let frame = frame.wrap_each(4, &Line::new("│ "), &Line::new(" │"));
        assert_eq!(frame.cursor().row, 0);
        assert_eq!(frame.cursor().col, 1 + 2);
        assert!(frame.cursor().is_visible);
    }

    #[test]
    fn wrap_each_materializes_fill_before_right_edge() {
        use crate::style::Style;
        use crossterm::style::Color;
        let line = Line::with_style("hi", Style::default().bg_color(Color::Blue)).with_fill(Color::Blue);
        let frame = Frame::new(vec![line]);
        let frame = frame.wrap_each(5, &Line::new("│ "), &Line::new(" │"));
        // Inner is padded to 5 cols ("hi   "), then borders surround it.
        assert_eq!(frame.lines()[0].plain_text(), "│ hi    │");
    }

    #[test]
    fn wrap_each_does_not_make_hidden_cursor_visible() {
        let frame = Frame::new(vec![Line::new("a")]);
        let frame = frame.wrap_each(3, &Line::new("│ "), &Line::new(" │"));
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn frame_part_fit_wraps_inner_to_slot_width() {
        let inner = Frame::new(vec![Line::new("abcdefgh")]);
        let part = FramePart::fit(inner, 3, FitOptions::wrap());
        assert_eq!(part.width, 3);
        assert_eq!(part.frame.lines().len(), 3);
        assert_eq!(part.frame.lines()[0].plain_text(), "abc");
    }

    #[test]
    fn frame_part_wrap_marks_rows_with_fill_metadata_when_bg_present() {
        use crate::style::Style;
        use crossterm::style::Color;
        let inner = Frame::new(vec![Line::with_style("abcdefgh", Style::default().bg_color(Color::Red))]);
        let part = FramePart::wrap(inner, 3);
        for line in part.frame.lines() {
            assert_eq!(line.fill(), Some(Color::Red), "wrap should mark each wrapped row with fill metadata");
        }
    }

    #[test]
    fn frame_part_truncate_clips_inner_to_slot_width() {
        let inner = Frame::new(vec![Line::new("abcdefgh"), Line::new("xy")]);
        let part = FramePart::truncate(inner, 3);
        assert_eq!(part.width, 3);
        assert_eq!(part.frame.lines().len(), 2);
        assert_eq!(part.frame.lines()[0].plain_text(), "abc");
        assert_eq!(part.frame.lines()[1].plain_text(), "xy");
    }

    #[test]
    fn frame_part_truncate_marks_rows_with_fill_metadata_when_bg_present() {
        use crate::style::Style;
        use crossterm::style::Color;
        let inner = Frame::new(vec![Line::with_style("abc", Style::default().bg_color(Color::Green))]);
        let part = FramePart::truncate(inner, 5);
        assert_eq!(part.frame.lines()[0].fill(), Some(Color::Green));
    }

    #[test]
    fn frame_part_wrap_then_hstack_composes_full_width_per_row() {
        let left = Frame::new(vec![Line::new("abcdefgh")]);
        let right = Frame::new(vec![Line::new("XX"), Line::new("YY"), Line::new("ZZ")]);
        let frame = Frame::hstack([FramePart::wrap(left, 3), FramePart::wrap(right, 2)]);
        assert_eq!(frame.lines().len(), 3);
        for line in frame.lines() {
            assert_eq!(line.display_width(), 5, "every composed row should be exactly slot_left + slot_right wide");
        }
        assert_eq!(frame.lines()[0].plain_text(), "abcXX");
        assert_eq!(frame.lines()[1].plain_text(), "defYY");
        assert_eq!(frame.lines()[2].plain_text(), "gh ZZ");
    }

    #[test]
    fn splice_inserts_after_row() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]);
        let other = Frame::new(vec![Line::new("X"), Line::new("Y")]);
        let frame = frame.splice(1, other);
        assert_eq!(frame.lines().len(), 5);
        assert_eq!(frame.lines()[0].plain_text(), "a");
        assert_eq!(frame.lines()[1].plain_text(), "b");
        assert_eq!(frame.lines()[2].plain_text(), "X");
        assert_eq!(frame.lines()[3].plain_text(), "Y");
        assert_eq!(frame.lines()[4].plain_text(), "c");
    }

    #[test]
    fn splice_at_end_appends() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b")]);
        let other = Frame::new(vec![Line::new("X")]);
        let frame = frame.splice(1, other);
        assert_eq!(frame.lines().len(), 3);
        assert_eq!(frame.lines()[2].plain_text(), "X");
    }

    #[test]
    fn splice_beyond_end_appends() {
        let frame = Frame::new(vec![Line::new("a")]);
        let other = Frame::new(vec![Line::new("X")]);
        let frame = frame.splice(100, other);
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[1].plain_text(), "X");
    }

    #[test]
    fn splice_empty_other_is_noop() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b")]).with_cursor(Cursor::visible(1, 0));
        let other = Frame::empty();
        let frame = frame.splice(0, other);
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.cursor(), Cursor::visible(1, 0));
    }

    #[test]
    fn splice_shifts_self_cursor_down() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]).with_cursor(Cursor::visible(2, 3));
        let other = Frame::new(vec![Line::new("X"), Line::new("Y"), Line::new("Z")]);
        let frame = frame.splice(1, other);
        assert_eq!(frame.cursor(), Cursor::visible(5, 3));
    }

    #[test]
    fn splice_preserves_self_cursor_before_insertion() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]).with_cursor(Cursor::visible(0, 1));
        let other = Frame::new(vec![Line::new("X")]);
        let frame = frame.splice(1, other);
        assert_eq!(frame.cursor(), Cursor::visible(0, 1));
    }

    #[test]
    fn splice_does_not_shift_self_cursor_on_insertion_row() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]).with_cursor(Cursor::visible(1, 0));
        let other = Frame::new(vec![Line::new("X")]);
        let frame = frame.splice(1, other);
        assert_eq!(frame.cursor(), Cursor::visible(1, 0));
    }

    #[test]
    fn splice_adopts_other_cursor() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]);
        let other = Frame::new(vec![Line::new("X"), Line::new("Y")]).with_cursor(Cursor::visible(1, 5));
        let frame = frame.splice(1, other);
        assert_eq!(frame.cursor(), Cursor::visible(3, 5));
    }

    #[test]
    fn splice_self_cursor_wins() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b")]).with_cursor(Cursor::visible(0, 0));
        let other = Frame::new(vec![Line::new("X")]).with_cursor(Cursor::visible(0, 5));
        let frame = frame.splice(0, other);
        assert_eq!(frame.cursor(), Cursor::visible(0, 0));
    }

    #[test]
    fn scroll_clips_to_viewport() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c"), Line::new("d"), Line::new("e")]);
        let frame = frame.scroll(1, 3);
        assert_eq!(frame.lines().len(), 3);
        assert_eq!(frame.lines()[0].plain_text(), "b");
        assert_eq!(frame.lines()[1].plain_text(), "c");
        assert_eq!(frame.lines()[2].plain_text(), "d");
    }

    #[test]
    fn scroll_adjusts_cursor() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c"), Line::new("d"), Line::new("e")])
            .with_cursor(Cursor::visible(3, 2));
        let frame = frame.scroll(1, 4);
        assert_eq!(frame.cursor(), Cursor::visible(2, 2));
    }

    #[test]
    fn scroll_hides_cursor_above_viewport() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]).with_cursor(Cursor::visible(0, 0));
        let frame = frame.scroll(2, 1);
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn scroll_hides_cursor_below_viewport() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c"), Line::new("d")])
            .with_cursor(Cursor::visible(3, 0));
        let frame = frame.scroll(0, 2);
        assert!(!frame.cursor().is_visible);
    }

    #[test]
    fn scroll_zero_offset_is_truncate() {
        let frame = Frame::new(vec![Line::new("a"), Line::new("b"), Line::new("c")]);
        let frame = frame.scroll(0, 2);
        assert_eq!(frame.lines().len(), 2);
        assert_eq!(frame.lines()[0].plain_text(), "a");
        assert_eq!(frame.lines()[1].plain_text(), "b");
    }
}
