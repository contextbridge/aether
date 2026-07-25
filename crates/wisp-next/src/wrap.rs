use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Splits a styled line across `width` columns, expanding tabs and honouring
/// embedded newlines. The span-aware counterpart to [`wrap_text_char`].
pub fn wrap_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
    let max_width = usize::from(width.max(1));
    let line_style = line.style;
    let alignment = line.alignment;
    let mut output = Vec::new();
    let mut spans = Vec::new();
    let mut current_width = 0;

    for span in line.spans {
        let mut fragment = String::new();
        for character in span.content.chars() {
            if character == '\n' {
                push_fragment(&mut spans, &mut fragment, span.style);
                output.push(make_line(std::mem::take(&mut spans), line_style, alignment));
                current_width = 0;
                continue;
            }
            if character == '\t' {
                let spaces = TAB_WIDTH - current_width % TAB_WIDTH;
                for _ in 0..spaces {
                    if current_width + 1 > max_width {
                        push_fragment(&mut spans, &mut fragment, span.style);
                        output.push(make_line(std::mem::take(&mut spans), line_style, alignment));
                        current_width = 0;
                    }
                    fragment.push(' ');
                    current_width += 1;
                }
                continue;
            }

            let character_width = character.width().unwrap_or(0);
            let (character, character_width) =
                if character_width > max_width { ('…', 1) } else { (character, character_width) };
            if current_width + character_width > max_width {
                push_fragment(&mut spans, &mut fragment, span.style);
                output.push(make_line(std::mem::take(&mut spans), line_style, alignment));
                current_width = 0;
            }
            fragment.push(character);
            current_width += character_width;
        }
        push_fragment(&mut spans, &mut fragment, span.style);
    }

    if !spans.is_empty() || output.is_empty() {
        output.push(make_line(spans, line_style, alignment));
    }
    output
}

/// Length in bytes, and display width, of the longest prefix of `text` that fits
/// within `max_width` columns.
///
/// Every width-bounded routine in this module is built on this walk so that
/// truncation, wrapping, and cursor placement can never disagree about where a
/// column falls.
pub fn fit_prefix(text: &str, max_width: usize) -> (usize, usize) {
    let mut width = 0;
    let mut bytes = 0;
    for (index, character) in text.char_indices() {
        let character_width = character.width().unwrap_or(0);
        if width + character_width > max_width {
            break;
        }
        width += character_width;
        bytes = index + character.len_utf8();
    }
    (bytes, width)
}

/// Truncates `spans` to `max_width` columns, appending an ellipsis when content
/// was dropped. Returns the spans unchanged when they already fit.
pub fn truncate_spans(spans: &[Span<'static>], max_width: usize) -> Vec<Span<'static>> {
    let display_width: usize = spans.iter().map(Span::width).sum();
    if display_width <= max_width {
        return spans.to_vec();
    }
    if max_width < ELLIPSIS_WIDTH {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut remaining = max_width - ELLIPSIS_WIDTH;
    for span in spans {
        if remaining == 0 {
            break;
        }
        let (bytes, width) = fit_prefix(&span.content, remaining);
        if bytes > 0 {
            result.push(Span::styled(span.content[..bytes].to_string(), span.style));
        }
        remaining -= width;
    }
    result.push(Span::raw(ELLIPSIS));
    result
}

/// Truncates `text` to `max_width` columns, appending an ellipsis when content
/// was dropped.
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let (bytes, _) = fit_prefix(text, max_width - ELLIPSIS_WIDTH);
    let mut result = text[..bytes].to_string();
    result.push_str(ELLIPSIS);
    result
}

/// Word-wraps `text` to `max_width` columns, breaking words that cannot fit.
pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw_line in text.split('\n') {
        if raw_line.width() <= max_width {
            lines.push(raw_line.to_string());
            continue;
        }
        let mut current = String::new();
        for word in raw_line.split(' ') {
            let needed = if current.is_empty() { word.width() } else { current.width() + 1 + word.width() };
            if needed <= max_width {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
            } else {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                let mut chunks = wrap_text_char(word, max_width);
                current = chunks.pop().unwrap_or_default();
                lines.append(&mut chunks);
            }
        }
        lines.push(current);
    }
    lines
}

/// Hard-wraps `text` at `max_width` columns, ignoring word boundaries. Always
/// returns at least one (possibly empty) line.
pub fn wrap_text_char(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (bytes, _) = fit_prefix(rest, max_width);
        // A single character wider than the budget would otherwise loop forever.
        let bytes = if bytes == 0 { rest.chars().next().map_or(rest.len(), char::len_utf8) } else { bytes };
        lines.push(rest[..bytes].to_string());
        rest = &rest[bytes..];
    }
    lines
}

/// Row and column a cursor sitting immediately after `prefix` occupies once the
/// text is hard-wrapped at `max_width`.
pub fn text_position_in_wrap(prefix: &str, max_width: usize) -> (usize, u16) {
    let lines = wrap_text_char(prefix, max_width);
    let row = lines.len().saturating_sub(1);
    let column = lines.last().map_or(0, |line| line.width());
    (row, u16::try_from(column).unwrap_or(u16::MAX))
}

/// Forces `line` to occupy exactly `width` columns, truncating with an ellipsis
/// or padding with `fill_style`.
pub fn fit_line(mut line: Line<'static>, width: usize, fill_style: Style) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if line.width() > width {
        let content_width = width.saturating_sub(ELLIPSIS_WIDTH).max(1);
        line = wrap_line(line, u16::try_from(content_width).unwrap_or(u16::MAX)).into_iter().next().unwrap_or_default();
        line.spans.push(Span::styled(ELLIPSIS, fill_style));
    }
    if line.width() < width {
        line.spans.push(Span::styled(" ".repeat(width - line.width()), fill_style));
    }
    line
}

/// `count` as a row or column count, saturating rather than wrapping on the
/// terminals nobody has.
pub fn rows(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

const TAB_WIDTH: usize = 4;
const ELLIPSIS: &str = "…";
const ELLIPSIS_WIDTH: usize = 1;

fn push_fragment(spans: &mut Vec<Span<'static>>, fragment: &mut String, style: Style) {
    if !fragment.is_empty() {
        spans.push(Span::styled(std::mem::take(fragment), style));
    }
}

fn make_line(spans: Vec<Span<'static>>, style: Style, alignment: Option<ratatui::layout::Alignment>) -> Line<'static> {
    Line { spans, style, alignment }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;
    use ratatui::text::Span;

    #[test]
    fn fit_prefix_respects_display_width_and_utf8_boundaries() {
        assert_eq!(fit_prefix("a界b", 0).0, 0);
        assert_eq!(fit_prefix("a界b", 1).0, 1);
        assert_eq!(fit_prefix("a界b", 2).0, 1);
        assert_eq!(fit_prefix("a界b", 3).0, 4);
        assert_eq!(fit_prefix("a界b", 4).0, 5);
    }

    #[test]
    fn fit_prefix_reports_the_width_it_consumed() {
        assert_eq!(fit_prefix("a界b", 3), (4, 3));
        assert_eq!(fit_prefix("a界b", 2), (1, 1), "a wide char that does not fit is excluded entirely");
    }

    #[test]
    fn truncate_spans_no_truncation_needed() {
        let spans = vec![Span::raw("hi")];
        let result = truncate_spans(&spans, 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "hi");
    }

    #[test]
    fn truncate_spans_adds_ellipsis() {
        let spans = vec![Span::raw("hello world")];
        let result = truncate_spans(&spans, 7);
        let total: usize = result.iter().map(Span::width).sum();
        assert!(total <= 7);
        assert!(result.iter().any(|s| s.content.contains('…')));
    }

    #[test]
    fn truncate_spans_narrow_width_returns_empty() {
        let spans = vec![Span::raw("abc")];
        let result = truncate_spans(&spans, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn truncate_to_width_never_exceeds_the_budget() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello", 0), "");
        assert!(truncate_to_width("hello world", 7).width() <= 7);
        assert!(truncate_to_width("界界界界", 5).width() <= 5);
    }

    #[test]
    fn wrap_text_preserves_existing_newlines() {
        let text = "line1\nline2\nline3";
        let result = wrap_text(text, 80);
        assert_eq!(result, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn wrap_text_word_breaks_long_line() {
        let text = "the quick brown fox jumps over";
        let result = wrap_text(text, 10);
        assert_eq!(result, vec!["the quick", "brown fox", "jumps over"]);
    }

    #[test]
    fn wrap_text_breaks_long_word_by_char() {
        let text = "supercalifragilistic";
        let result = wrap_text(text, 5);
        assert!(!result.iter().any(|line| line.width() > 5));
    }

    #[test]
    fn wrap_text_char_wraps_by_character() {
        let text = "abcdefghij";
        let result = wrap_text_char(text, 3);
        assert_eq!(result, vec!["abc", "def", "ghi", "j"]);
    }

    #[test]
    fn wrap_text_char_empty_input() {
        assert_eq!(wrap_text_char("", 10), vec![String::new()]);
        assert_eq!(wrap_text_char("text", 0), vec![String::new()]);
    }

    #[test]
    fn wrap_text_char_makes_progress_on_oversized_characters() {
        assert_eq!(wrap_text_char("界界", 1), vec!["界", "界"]);
    }

    #[test]
    fn text_position_in_wrap_single_line() {
        let (line, col) = text_position_in_wrap("hello", 10);
        assert_eq!(line, 0);
        assert_eq!(col, 5);
    }

    #[test]
    fn text_position_in_wrap_multi_line() {
        let (line, col) = text_position_in_wrap("abcdefghijkl", 5);
        assert_eq!(line, 2);
        assert_eq!(col, 2);
    }

    #[test]
    fn text_position_in_wrap_agrees_with_wrap_text_char() {
        let text = "the quick brown fox";
        for width in 1..12 {
            let (row, column) = text_position_in_wrap(text, width);
            let wrapped = wrap_text_char(text, width);
            assert_eq!(row, wrapped.len() - 1, "row disagrees at width {width}");
            assert_eq!(usize::from(column), wrapped[row].width(), "column disagrees at width {width}");
        }
    }

    #[test]
    fn fit_line_truncates_long_line() {
        let line = Line::from("hello world");
        let result = fit_line(line, 5, Style::new());
        assert!(result.width() <= 5);
        assert!(result.spans.iter().any(|s| s.content.contains('…')));
    }

    #[test]
    fn fit_line_pads_short_line() {
        let line = Line::from("hi");
        let result = fit_line(line, 10, Style::new());
        assert_eq!(result.width(), 10);
    }

    #[test]
    fn fit_line_zero_width_returns_default() {
        let result = fit_line(Line::from("hi"), 0, Style::new());
        assert_eq!(result.width(), 0);
    }
}
