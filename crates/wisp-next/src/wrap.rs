use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Splits a styled line across `width` columns, expanding tabs, honouring
/// embedded newlines, and preferring word boundaries. Long words still wrap at
/// the available width.
pub fn wrap_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
    let max_width = usize::from(width.max(1));
    let characters = line_characters(line.spans, max_width);

    let mut output = Vec::new();
    let mut row_start = 0;
    loop {
        let (row_end, next_row_start) = next_wrapped_row(&characters, row_start, max_width);
        output.push(make_line(characters_to_spans(&characters[row_start..row_end]), line.style, line.alignment));
        if next_row_start >= characters.len() {
            return output;
        }
        row_start = next_row_start;
    }
}

/// Length in bytes, and display width, of the longest prefix of `text` that fits
/// within `max_width` columns.
///
/// Truncation and cursor placement are both built on this walk so that they can
/// never disagree about where a column falls.
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

/// Truncates `spans` to `max_width` columns, appending an ellipsis in
/// `ellipsis_style` when content was dropped. Returns the spans unchanged when
/// they already fit.
pub fn truncate_spans(spans: &[Span<'static>], max_width: usize, ellipsis_style: Style) -> Vec<Span<'static>> {
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
    result.push(Span::styled(ELLIPSIS, ellipsis_style));
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

/// Word-wraps `text` to `max_width` columns, breaking words that cannot fit. The
/// plain-text view of [`wrap_line`], so styled and unstyled content can never
/// wrap differently.
pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    wrap_line(Line::from(Span::raw(text.to_string())), rows(max_width))
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect()
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
        line.spans = truncate_spans(&line.spans, width, fill_style);
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

struct WrappedCharacter {
    character: char,
    width: usize,
    style: Style,
    breakable_whitespace: bool,
}

fn line_characters(spans: Vec<Span<'static>>, max_width: usize) -> Vec<WrappedCharacter> {
    let mut characters = Vec::new();
    let mut source_column = 0;
    for span in spans {
        for character in span.content.chars() {
            match character {
                '\n' => {
                    characters.push(WrappedCharacter {
                        character,
                        width: 0,
                        style: span.style,
                        breakable_whitespace: false,
                    });
                    source_column = 0;
                }
                '\t' => {
                    let spaces = TAB_WIDTH - source_column % TAB_WIDTH;
                    // Indentation is not a break opportunity: breaking inside it
                    // would emit a row holding nothing but leading whitespace.
                    characters.extend((0..spaces).map(|_| WrappedCharacter {
                        character: ' ',
                        width: 1,
                        style: span.style,
                        breakable_whitespace: false,
                    }));
                    source_column += spaces;
                }
                _ => {
                    let width = character.width().unwrap_or(0);
                    let (character, width) =
                        if width > max_width { ('…', ELLIPSIS_WIDTH) } else { (character, width) };
                    characters.push(WrappedCharacter {
                        character,
                        width,
                        style: span.style,
                        breakable_whitespace: character.is_whitespace(),
                    });
                    source_column += width;
                }
            }
        }
    }
    characters
}

/// The half-open range of `characters` forming the row that starts at
/// `row_start`, and the index the row after it starts at. A next-start at or
/// past the end means there is no further row.
fn next_wrapped_row(characters: &[WrappedCharacter], row_start: usize, max_width: usize) -> (usize, usize) {
    let mut row_width = 0;
    let mut last_whitespace = None;

    for (index, character) in characters.iter().enumerate().skip(row_start) {
        if character.character == '\n' {
            return (index, index + 1);
        }

        if character.width > 0 && row_width + character.width > max_width && row_width > 0 {
            return if character.breakable_whitespace {
                (index, index + 1)
            } else if let Some(whitespace) = last_whitespace {
                (whitespace, whitespace + 1)
            } else {
                (index, index)
            };
        }

        row_width += character.width;
        if character.breakable_whitespace {
            last_whitespace = Some(index);
        }
    }

    (characters.len(), characters.len())
}

fn characters_to_spans(characters: &[WrappedCharacter]) -> Vec<Span<'static>> {
    characters
        .chunk_by(|left, right| left.style == right.style)
        .map(|run| Span::styled(run.iter().map(|character| character.character).collect::<String>(), run[0].style))
        .collect()
}

fn make_line(spans: Vec<Span<'static>>, style: Style, alignment: Option<ratatui::layout::Alignment>) -> Line<'static> {
    Line { spans, style, alignment }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Alignment;
    use ratatui::style::{Color, Style};
    use ratatui::text::Span;

    /// A line that keeps its newlines: `Line::raw` splits them into separate
    /// spans and drops the newlines themselves.
    fn raw_line(text: &str) -> Line<'static> {
        Line::from(Span::raw(text.to_string()))
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn wrapped(line: Line<'static>, width: u16) -> Vec<String> {
        wrap_line(line, width).iter().map(line_text).collect()
    }

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
        let result = truncate_spans(&spans, 10, Style::new());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "hi");
    }

    #[test]
    fn truncate_spans_adds_ellipsis() {
        let spans = vec![Span::raw("hello world")];
        let result = truncate_spans(&spans, 7, Style::new());
        let total: usize = result.iter().map(Span::width).sum();
        assert!(total <= 7);
        assert!(result.iter().any(|s| s.content.contains('…')));
    }

    #[test]
    fn truncate_spans_narrow_width_returns_empty() {
        let spans = vec![Span::raw("abc")];
        let result = truncate_spans(&spans, 0, Style::new());
        assert!(result.is_empty());
    }

    #[test]
    fn truncate_spans_styles_the_ellipsis_for_the_caller() {
        let spans = vec![Span::raw("hello world")];
        let result = truncate_spans(&spans, 7, Style::new().fg(Color::Red));
        let ellipsis = result.last().expect("truncation appends an ellipsis");
        assert_eq!(ellipsis.content, "…");
        assert_eq!(ellipsis.style.fg, Some(Color::Red));
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
    fn wrap_text_agrees_with_wrap_line() {
        let text = "the quick brown\tfox jumps over the lazy dog";
        for width in 1..24 {
            assert_eq!(wrap_text(text, width), wrapped(raw_line(text), rows(width)), "disagree at width {width}");
        }
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
    fn wrap_line_wraps_at_word_boundary() {
        assert_eq!(wrapped(raw_line("hello world"), 7), ["hello", "world"]);
    }

    #[test]
    fn wrap_line_fills_each_row_with_as_many_words_as_fit() {
        assert_eq!(wrapped(raw_line("hello world foo"), 12), ["hello world", "foo"]);
    }

    #[test]
    fn wrap_line_falls_back_to_char_break_without_whitespace() {
        assert_eq!(wrapped(raw_line("superlongword next"), 5), ["super", "longw", "ord", "next"]);
    }

    #[test]
    fn wrap_line_breaks_at_whitespace_across_span_boundaries() {
        let line = Line::from(vec![
            Span::styled("@aaaaa", Style::new().fg(Color::Red)),
            Span::raw(" "),
            Span::styled("@bbbbbb", Style::new().fg(Color::Blue)),
        ]);

        let rows = wrap_line(line, 10);

        assert_eq!(rows.iter().map(line_text).collect::<Vec<_>>(), ["@aaaaa", "@bbbbbb"]);
        assert_eq!(rows[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(rows[1].spans[0].style.fg, Some(Color::Blue));
    }

    #[test]
    fn wrap_line_drops_whitespace_when_a_new_span_starts_at_the_wrap_boundary() {
        let line = Line::from(vec![
            Span::styled("abcdefghij", Style::new().fg(Color::Red)),
            Span::styled(" klm", Style::new().fg(Color::Blue)),
        ]);

        let rows = wrap_line(line, 10);

        assert_eq!(rows.iter().map(line_text).collect::<Vec<_>>(), ["abcdefghij", "klm"]);
        assert_eq!(rows[1].spans[0].style.fg, Some(Color::Blue));
    }

    #[test]
    fn wrap_line_hard_wraps_a_long_styled_token_without_whitespace() {
        let line = Line::from(Span::styled("@abcdefghijk", Style::new().fg(Color::Green)));

        let rows = wrap_line(line, 5);

        assert_eq!(rows.iter().map(line_text).collect::<Vec<_>>(), ["@abcd", "efghi", "jk"]);
        assert!(rows.iter().all(|row| row.spans[0].style.fg == Some(Color::Green)));
    }

    #[test]
    fn wrap_line_honours_embedded_newlines() {
        assert_eq!(wrapped(raw_line("a\nb"), 10), ["a", "b"]);
        assert_eq!(wrapped(raw_line("a\n\nb"), 10), ["a", "", "b"], "a blank source line stays a blank row");
    }

    #[test]
    fn wrap_line_does_not_add_a_row_for_a_trailing_newline() {
        assert_eq!(wrapped(raw_line("a\n"), 10), ["a"]);
    }

    #[test]
    fn wrap_line_keeps_an_empty_line_as_one_blank_row() {
        assert_eq!(wrapped(Line::default(), 10), [""]);
        assert_eq!(wrapped(raw_line(""), 10), [""]);
    }

    #[test]
    fn wrap_line_expands_tabs_without_breaking_inside_indentation() {
        assert_eq!(wrapped(raw_line("\tabc def"), 6), ["    ab", "c def"]);
    }

    #[test]
    fn wrap_line_replaces_characters_wider_than_the_whole_row() {
        assert_eq!(wrapped(raw_line("界界界"), 1), ["…", "…", "…"]);
    }

    #[test]
    fn wrap_line_merges_adjacent_spans_that_share_a_style() {
        let style = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::styled("hello ", style), Span::styled("world", style)]);

        let rows = wrap_line(line, 20);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spans.len(), 1);
        assert_eq!(rows[0].spans[0].content, "hello world");
    }

    #[test]
    fn wrap_line_carries_line_style_and_alignment_onto_every_row() {
        let line = Line {
            spans: vec![Span::raw("hello world")],
            style: Style::new().fg(Color::Red),
            alignment: Some(Alignment::Center),
        };

        let rows = wrap_line(line, 7);

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.style.fg == Some(Color::Red)));
        assert!(rows.iter().all(|row| row.alignment == Some(Alignment::Center)));
    }

    #[test]
    fn wrap_line_never_exceeds_the_available_width() {
        let line = raw_line("界 the quick\tbrown fox jumps");
        for width in 1..16u16 {
            for row in wrap_line(line.clone(), width) {
                assert!(row.width() <= usize::from(width), "{:?} exceeds width {width}", line_text(&row));
            }
        }
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
        let result = fit_line(Line::from("hello world"), 5, Style::new());
        assert_eq!(line_text(&result), "hell…");
        assert_eq!(result.width(), 5);
    }

    #[test]
    fn fit_line_truncates_at_the_overflowing_character_not_the_word() {
        let result = fit_line(Line::from("hello world"), 8, Style::new());
        assert_eq!(line_text(&result), "hello w…", "truncation fills the row rather than wrapping to a word");
        assert_eq!(result.width(), 8);
    }

    #[test]
    fn fit_line_styles_the_ellipsis_with_the_fill() {
        let fill = Style::new().fg(Color::Red);
        let result = fit_line(Line::from("hello world"), 8, fill);
        assert_eq!(result.spans.last().map(|span| span.style.fg), Some(Some(Color::Red)));
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
