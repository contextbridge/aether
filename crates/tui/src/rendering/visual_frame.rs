use crate::Frame;
use crate::rendering::render_context::Size;

use super::frame::{Cursor, FitOptions};
use super::line::Line;

/// Result of diffing two `VisualFrames`' visible lines.
#[derive(Debug)]
pub struct LineDiff<'a> {
    /// Row index to start rewriting from.
    pub rewrite_from: usize,
    /// Lines to write starting at `rewrite_from`.
    pub lines: &'a [Line],
    /// Number of visible rows in the previous frame.
    pub previous_row_count: usize,
}

/// Terminal-ready visual frame prepared for the terminal.
///
/// This is the pure mapping step from logical output (`Frame`) to terminal-ready
/// visual state. It is responsible for:
/// - clamping cursor to logical content
/// - applying final soft-wrap using terminal width
/// - remapping logical cursor position to visual row/col after wrapping
/// - splitting prepared output into scrollback vs visible lines
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualFrame {
    scrollback_lines: Vec<Line>,
    visible_lines: Vec<Line>,
    cursor: Cursor,
}

impl VisualFrame {
    /// Creates a `VisualFrame` from a logical Frame, applying soft-wrap and viewport split.
    ///
    /// As the terminal-facing layer, this is also where deferred row-fill
    /// metadata gets materialized into trailing spaces sized to the terminal
    /// width. Materializing here (rather than during composition) lets fills
    /// survive intermediate wraps without producing phantom rows.
    pub fn from_frame(frame: Frame, size: Size) -> Self {
        let was_cursor_visible = frame.cursor().is_visible;
        let fitted = frame.fit(size.width, FitOptions::wrap());
        let (mut wrapped_lines, fitted_cursor) = fitted.into_parts();

        if size.width > 0 {
            let target = usize::from(size.width);
            for line in &mut wrapped_lines {
                if line.fill().is_some() {
                    line.extend_bg_to_width(target);
                }
            }
        }

        // Frame::fit hides the cursor at width == 0; preserve the caller's visibility.
        let visual_cursor_col = if size.width == 0 { 0 } else { fitted_cursor.col };
        let visual_cursor_row = if size.width == 0 { 0 } else { fitted_cursor.row };

        let viewport_rows = usize::from(size.height.max(1));
        let overflow = wrapped_lines.len().saturating_sub(viewport_rows);
        let visible_lines = wrapped_lines.split_off(overflow);
        let final_cursor_row = visual_cursor_row.saturating_sub(overflow).min(visible_lines.len().saturating_sub(1));

        Self {
            scrollback_lines: wrapped_lines,
            visible_lines,
            cursor: Cursor { row: final_cursor_row, col: visual_cursor_col, is_visible: was_cursor_visible },
        }
    }

    pub fn scrollback_lines(&self) -> &[Line] {
        &self.scrollback_lines
    }

    pub fn visible_lines(&self) -> &[Line] {
        &self.visible_lines
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Diff `prev`'s visible lines against `next`, returning the minimal rewrite needed.
    /// `prev = None` is treated as an empty previous frame. Returns `None` if visible lines are identical.
    pub fn diff<'a>(prev: Option<&VisualFrame>, next: &'a VisualFrame) -> Option<LineDiff<'a>> {
        let prev_lines = prev.map_or(&[][..], |p| p.visible_lines.as_slice());
        let next_lines = &next.visible_lines;
        if next_lines.as_slice() == prev_lines {
            return None;
        }

        let first_diff = prev_lines
            .iter()
            .zip(next_lines.iter())
            .position(|(old, new)| old != new)
            .unwrap_or(prev_lines.len().min(next_lines.len()));

        let rewrite_from = if next_lines.is_empty() { 0 } else { first_diff.min(next_lines.len() - 1) };

        Some(LineDiff { rewrite_from, lines: &next_lines[rewrite_from..], previous_row_count: prev_lines.len() })
    }
}

/// Prepare logical lines for scrollback using the same width semantics as `VisualFrame`.
pub fn prepare_lines_for_scrollback(lines: &[Line], width: u16) -> Vec<Line> {
    lines.iter().flat_map(|line| super::soft_wrap::soft_wrap_line(line, width)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::frame::Frame;

    #[test]
    fn visual_frame_from_frame_soft_wraps_and_splits() {
        let frame = Frame::new(vec![Line::new("abcdef")]).with_cursor(Cursor { row: 0, col: 5, is_visible: true });

        let visual = VisualFrame::from_frame(frame, Size::from((3, 5)));
        assert_eq!(visual.visible_lines(), &[Line::new("abc"), Line::new("def")]);
        assert_eq!(visual.cursor().row, 1);
        assert_eq!(visual.cursor().col, 2);
    }

    #[test]
    fn visual_frame_splits_overflow_from_visible_lines() {
        let frame = Frame::new(vec![Line::new("L1"), Line::new("L2"), Line::new("L3"), Line::new("L4")])
            .with_cursor(Cursor { row: 3, col: 0, is_visible: true });

        let visual = VisualFrame::from_frame(frame, Size::from((80, 2)));
        assert_eq!(visual.scrollback_lines(), &[Line::new("L1"), Line::new("L2")]);
        assert_eq!(visual.visible_lines(), &[Line::new("L3"), Line::new("L4")]);
        assert_eq!(visual.cursor().row, 1);
        assert_eq!(visual.cursor().col, 0);
    }

    #[test]
    fn visual_frame_cursor_in_scrollback_gets_clamped() {
        let frame = Frame::new(vec![Line::new("L1"), Line::new("L2"), Line::new("L3")]).with_cursor(Cursor {
            row: 0,
            col: 0,
            is_visible: true,
        });

        let visual = VisualFrame::from_frame(frame, Size::from((80, 2)));
        assert_eq!(visual.cursor().row, 0);
        assert_eq!(visual.visible_lines().len(), 2);
    }

    #[test]
    fn visual_frame_empty_frame() {
        let frame = Frame::new(vec![]);
        let visual = VisualFrame::from_frame(frame, Size::from((80, 24)));
        assert!(visual.scrollback_lines().is_empty());
        assert!(visual.visible_lines().is_empty());
    }

    #[test]
    fn visual_frame_materializes_fill_to_terminal_width() {
        use crate::style::Style;
        use crossterm::style::Color;

        // A row marked with fill but no trailing spaces should be materialized
        // to the terminal width when VisualFrame::from_frame runs.
        let line = Line::with_style("hi", Style::default().bg_color(Color::Blue)).with_fill(Color::Blue);
        let frame = Frame::new(vec![line]);

        let visual = VisualFrame::from_frame(frame, Size::from((6, 1)));
        let visible = visual.visible_lines();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].plain_text(), "hi    ");
        assert_eq!(visible[0].fill(), None, "fill should be cleared after materialization");
    }

    #[test]
    fn fill_marked_row_does_not_produce_phantom_rows_when_wrapped_smaller() {
        use crate::style::Style;
        use crossterm::style::Color;

        // Regression for the trailing-space wrap artifact: when a row was
        // pre-padded with `extend_bg_to_width(30)` and then wrapped at width 10,
        // the trailing 25 spaces would themselves wrap into 3 phantom rows.
        // With fill metadata, the row's actual content is "ab" → 1 wrapped
        // row at width 10, materialized to 10 columns by VisualFrame.
        let line = Line::with_style("ab", Style::default().bg_color(Color::Red)).with_fill(Color::Red);
        let frame = Frame::new(vec![line]);

        let visual = VisualFrame::from_frame(frame, Size::from((10, 5)));
        assert_eq!(visual.visible_lines().len(), 1, "fill should not produce phantom wrapped rows");
        assert_eq!(visual.visible_lines()[0].plain_text(), "ab        ");
    }

    #[test]
    fn visual_frame_zero_width_keeps_lines_unwrapped() {
        let frame = Frame::new(vec![Line::new("abcdef")]).with_cursor(Cursor { row: 0, col: 3, is_visible: true });

        let visual = VisualFrame::from_frame(frame, Size::from((0, 5)));
        assert_eq!(visual.visible_lines(), &[Line::new("abcdef")]);
        assert_eq!(visual.cursor().col, 0);
    }

    #[test]
    fn prepare_lines_for_scrollback_matches_visual_frame_wrapping() {
        let lines = vec![Line::new("abcdef")];
        let visual_frame_lines = {
            let frame = Frame::new(lines.clone()).with_cursor(Cursor { row: 0, col: 0, is_visible: true });
            let visual = VisualFrame::from_frame(frame, Size::from((3, 5)));
            visual.visible_lines().to_vec()
        };

        let scrollback_lines = prepare_lines_for_scrollback(&lines, 3);
        assert_eq!(visual_frame_lines, scrollback_lines);
    }

    fn visual(lines: &[&str]) -> VisualFrame {
        let frame = Frame::new(lines.iter().map(|l| Line::new(*l)).collect()).with_cursor(Cursor {
            row: lines.len().saturating_sub(1),
            col: 0,
            is_visible: true,
        });
        VisualFrame::from_frame(frame, Size::from((80, 24)))
    }

    #[test]
    fn diff_identical_frames_returns_none() {
        let a = visual(&["hello", "world"]);
        let b = visual(&["hello", "world"]);
        assert!(VisualFrame::diff(Some(&a), &b).is_none());
    }

    #[test]
    fn diff_empty_to_nonempty_returns_full_rewrite() {
        let b = visual(&["hello", "world"]);
        let diff = VisualFrame::diff(None, &b).unwrap();
        assert_eq!(diff.rewrite_from, 0);
        assert_eq!(diff.lines.len(), 2);
        assert_eq!(diff.previous_row_count, 0);
    }

    #[test]
    fn diff_changed_middle_line() {
        let a = visual(&["aaa", "bbb", "ccc"]);
        let b = visual(&["aaa", "BBB", "ccc"]);
        let diff = VisualFrame::diff(Some(&a), &b).unwrap();
        assert_eq!(diff.rewrite_from, 1);
        assert_eq!(diff.lines.len(), 2);
        assert_eq!(diff.previous_row_count, 3);
    }

    #[test]
    fn diff_appended_line() {
        let a = visual(&["aaa", "bbb"]);
        let b = visual(&["aaa", "bbb", "ccc"]);
        let diff = VisualFrame::diff(Some(&a), &b).unwrap();
        assert_eq!(diff.rewrite_from, 2);
        assert_eq!(diff.lines.len(), 1);
        assert_eq!(diff.previous_row_count, 2);
    }
}
