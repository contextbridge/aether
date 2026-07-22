use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

pub fn byte_at_display_column(text: &str, column: usize) -> usize {
    let mut width = 0;
    let mut byte = 0;
    for (index, character) in text.char_indices() {
        let character_width = character.width().unwrap_or(0);
        if width + character_width > column {
            break;
        }
        width += character_width;
        byte = index + character.len_utf8();
    }
    byte
}

pub fn truncate_spans(spans: &[Span<'static>], max_width: usize) -> Vec<Span<'static>> {
    let display_width: usize = spans.iter().map(Span::width).sum();
    if display_width <= max_width {
        return spans.to_vec();
    }
    let ellipsis = "…";
    let ellipsis_width = 1;
    if max_width < ellipsis_width {
        return Vec::new();
    }
    let budget = max_width - ellipsis_width;
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut remaining = budget;
    for span in spans {
        if remaining == 0 {
            break;
        }
        let text = &span.content;
        let style = span.style;
        let mut byte_end = 0;
        let mut col = 0;
        for (i, ch) in text.char_indices() {
            let cw = ch.width().unwrap_or(0);
            if col + cw > remaining {
                break;
            }
            col += cw;
            byte_end = i + ch.len_utf8();
        }
        if byte_end > 0 {
            result.push(Span::styled(text[..byte_end].to_string(), style));
        }
        remaining -= col;
    }
    result.push(Span::raw(ellipsis));
    result
}

pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let budget = max_width.saturating_sub(1);
    let mut result = String::new();
    let mut current_width = 0;
    for ch in text.chars() {
        let char_width = ch.width().unwrap_or(0);
        if current_width + char_width > budget {
            break;
        }
        result.push(ch);
        current_width += char_width;
    }
    result.push('…');
    result
}

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
                current = break_long_word(word, max_width, &mut lines);
            }
        }
        lines.push(current);
    }
    lines
}

pub fn wrap_text_char(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width > max_width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub fn text_position_in_wrap(prefix: &str, max_width: usize) -> (usize, u16) {
    let mut line = 0usize;
    let mut current_width = 0usize;
    for ch in prefix.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width > max_width && current_width > 0 {
            line += 1;
            current_width = 0;
        }
        current_width += ch_width;
    }
    (line, u16::try_from(current_width).unwrap_or(u16::MAX))
}

pub fn fit_line(mut line: Line<'static>, width: usize, fill_style: Style) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if line.width() > width {
        let content_width = width.saturating_sub(1).max(1);
        line = wrap_line(line, u16::try_from(content_width).unwrap_or(u16::MAX)).into_iter().next().unwrap_or_default();
        line.spans.push(Span::styled("…", fill_style));
    }
    if line.width() < width {
        line.spans.push(Span::styled(" ".repeat(width - line.width()), fill_style));
    }
    line
}

const TAB_WIDTH: usize = 4;

fn push_fragment(spans: &mut Vec<Span<'static>>, fragment: &mut String, style: Style) {
    if !fragment.is_empty() {
        spans.push(Span::styled(std::mem::take(fragment), style));
    }
}

fn make_line(spans: Vec<Span<'static>>, style: Style, alignment: Option<ratatui::layout::Alignment>) -> Line<'static> {
    Line { spans, style, alignment }
}

fn break_long_word(word: &str, max_width: usize, lines: &mut Vec<String>) -> String {
    let mut current = String::new();
    for character in word.chars() {
        if current.width() + character.width().unwrap_or(0) > max_width {
            lines.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;
    use ratatui::text::Span;

    #[test]
    fn byte_position_respects_display_width_and_utf8_boundaries() {
        assert_eq!(byte_at_display_column("a界b", 0), 0);
        assert_eq!(byte_at_display_column("a界b", 1), 1);
        assert_eq!(byte_at_display_column("a界b", 2), 1);
        assert_eq!(byte_at_display_column("a界b", 3), 4);
        assert_eq!(byte_at_display_column("a界b", 4), 5);
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
