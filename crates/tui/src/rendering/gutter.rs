use super::frame::{FitOptions, Frame};
use super::line::Line;

/// Number of decimal digits in `value`, minimum 1 (so `0` returns `1`).
/// Useful for sizing gutters that show line numbers.
pub fn digit_count(value: usize) -> usize {
    value.checked_ilog10().map_or(1, |d| d as usize + 1)
}

/// Soft-wrap `content` to fit inside `total_width` columns and prepend a
/// gutter: the first visual row gets `head`, continuation rows get `tail`.
/// `head` and `tail` must have equal display width (debug-asserted by
/// [`Frame::prefix`]); that width is reserved for the gutter and the remainder
/// is used for content.
///
/// Each wrapped row has its background extended to the inner width before the
/// gutter is prepended, so row-fill (e.g. diff added/removed stripes) spans
/// the full content column without bleeding into the gutter column.
pub fn wrap_with_gutter(content: Line, total_width: u16, head: &Line, tail: &Line) -> Frame {
    let gutter_width = u16::try_from(head.display_width()).unwrap_or(u16::MAX);
    let inner_width = total_width.saturating_sub(gutter_width);
    let inner_width_usize = usize::from(inner_width);
    Frame::new(vec![content])
        .fit(inner_width, FitOptions::wrap())
        .map_lines(|mut line| {
            line.extend_bg_to_width(inner_width_usize);
            line
        })
        .prefix(head, tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::style::Style;
    use crossterm::style::Color;

    fn head_tail(width: usize) -> (Line, Line) {
        let head = Line::with_style(format!("{:>width$}", 7, width = width), Style::default());
        let tail = Line::new(" ".repeat(width));
        (head, tail)
    }

    #[test]
    fn short_content_produces_single_row_with_head() {
        let (head, tail) = head_tail(2);
        let content = Line::new("hi");
        let frame = wrap_with_gutter(content, 10, &head, &tail);
        let lines = frame.into_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().starts_with(" 7"), "expected head on first row: {:?}", lines[0].plain_text());
    }

    #[test]
    fn long_content_wraps_and_tail_prefixes_continuations() {
        let (head, tail) = head_tail(2);
        let long = "x".repeat(30);
        let content = Line::new(long);
        let frame = wrap_with_gutter(content, 10, &head, &tail);
        let lines = frame.into_lines();
        assert!(lines.len() >= 2, "expected wrap into multiple rows, got {}", lines.len());
        assert!(lines[0].plain_text().starts_with(" 7"), "row 0 should start with head");
        for (i, line) in lines.iter().enumerate().skip(1) {
            assert!(
                line.plain_text().starts_with("  "),
                "row {i} should start with tail spaces, got {:?}",
                line.plain_text()
            );
        }
    }

    #[test]
    fn content_background_is_extended_but_does_not_cover_gutter() {
        let head = Line::with_style("77 ", Style::default());
        let tail = Line::new("   ");
        let bg = Color::Rgb { r: 10, g: 20, b: 30 };
        let content = Line::with_style("short", Style::default().bg_color(bg));
        let frame = wrap_with_gutter(content, 20, &head, &tail);
        let lines = frame.into_lines();
        assert_eq!(lines.len(), 1);
        let spans = lines[0].spans();
        assert_eq!(spans[0].style().bg, None, "gutter span should not carry content bg");
        let any_content_bg = spans.iter().skip(1).any(|s| s.style().bg == Some(bg));
        assert!(any_content_bg, "content spans should retain their bg");
    }

    #[test]
    #[should_panic(expected = "head and tail must have equal display width")]
    fn head_tail_width_mismatch_panics_in_debug() {
        let head = Line::new("12");
        let tail = Line::new("   ");
        let _ = wrap_with_gutter(Line::new("hi"), 10, &head, &tail);
    }

    #[test]
    fn digit_count_works() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(1), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(99), 2);
        assert_eq!(digit_count(100), 3);
        assert_eq!(digit_count(999), 3);
    }
}
