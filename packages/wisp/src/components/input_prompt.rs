use std::ops::Range;
use tui::{Line, ViewContext};

pub fn prompt_content_width(terminal_width: usize) -> usize {
    terminal_width.saturating_sub(2).max(1)
}

pub fn prompt_text_start_col(_terminal_width: usize) -> usize {
    2
}

pub struct InputPrompt<'a> {
    pub input: &'a str,
    pub cursor_index: usize,
    pub highlight_range: Option<Range<usize>>,
}

pub struct InputPromptLayout {
    pub lines: Vec<Line>,
    /// Cursor row within `lines` (0-based).
    pub cursor_row: usize,
    /// Cursor column on that row (0-based).
    pub cursor_col: u16,
}

impl InputPrompt<'_> {
    pub fn layout(&self, context: &ViewContext) -> InputPromptLayout {
        let width = usize::from(context.size.width);
        let cursor_index = clamp_to_char_boundary(self.input, self.cursor_index);
        let styled_input = style_input(self.input, context, self.highlight_range.as_ref());

        let content_width = prompt_content_width(width);
        let content_width_u16 = u16::try_from(content_width).unwrap_or(u16::MAX);
        let wrapped_chunks = styled_input.soft_wrap(content_width_u16);

        let (cursor_content_row, cursor_content_col) =
            wrapped_cursor_position(self.input, cursor_index, content_width_u16);

        let content_rows = wrapped_chunks.len().max(cursor_content_row + 1);

        let mut lines = Vec::with_capacity(content_rows + 2);
        lines.push(Line::styled("─".repeat(width), context.theme.muted()));

        for row in 0..content_rows {
            let chunk = wrapped_chunks.get(row).cloned().unwrap_or_default();
            let mut middle = Line::default();
            if row == 0 {
                middle.push_styled("> ", context.theme.primary());
            } else {
                middle.push_styled("  ", context.theme.muted());
            }
            middle.append_line(&chunk);
            lines.push(middle);
        }

        lines.push(Line::styled("─".repeat(width), context.theme.muted()));

        InputPromptLayout {
            lines,
            cursor_row: 1 + cursor_content_row,
            cursor_col: u16::try_from(prompt_text_start_col(width) + cursor_content_col).unwrap_or(u16::MAX),
        }
    }
}

impl InputPrompt<'_> {
    #[cfg(test)]
    pub fn render(&self, context: &ViewContext) -> Vec<Line> {
        self.layout(context).lines
    }
}

fn style_input(input: &str, context: &ViewContext, highlight: Option<&Range<usize>>) -> Line {
    let highlight = highlight
        .filter(|r| r.start < r.end && r.end <= input.len())
        .filter(|r| input.is_char_boundary(r.start) && input.is_char_boundary(r.end));

    if highlight.is_none() && !input.contains('@') {
        return Line::styled(input, context.theme.text_primary());
    }

    let mentions = mention_ranges(input);
    let base = context.theme.text_primary();
    let info = context.theme.info();
    let warning = context.theme.warning();

    let color_at = |byte: usize| -> tui::Color {
        if let Some(r) = &highlight
            && r.contains(&byte)
        {
            return warning;
        }
        if mentions.iter().any(|m| m.contains(&byte)) {
            return info;
        }
        base
    };

    let mut line = Line::default();
    let mut run_start = 0;
    let mut current = color_at(0);
    for (i, _) in input.char_indices().skip(1) {
        let c = color_at(i);
        if c != current {
            line.push_styled(&input[run_start..i], current);
            run_start = i;
            current = c;
        }
    }
    line.push_styled(&input[run_start..], current);
    line
}

fn mention_ranges(input: &str) -> Vec<Range<usize>> {
    if !input.contains('@') {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut last_end = 0;
    for (at_pos, _) in input.match_indices('@') {
        if at_pos < last_end {
            continue;
        }
        let end = input[at_pos..].find(char::is_whitespace).map_or(input.len(), |i| at_pos + i);
        out.push(at_pos..end);
        last_end = end;
    }
    out
}

fn clamp_to_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while !text.is_char_boundary(idx) {
        idx = idx.saturating_sub(1);
    }
    idx
}

fn wrapped_cursor_position(input: &str, cursor_index: usize, content_width: u16) -> (usize, usize) {
    let cursor_index = clamp_to_char_boundary(input, cursor_index);
    let wrapped_prefix = Line::new(&input[..cursor_index]).soft_wrap(content_width);
    let row = wrapped_prefix.len().saturating_sub(1);
    let col = wrapped_prefix.last().map_or(0, Line::display_width);
    (row, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_three_lines() {
        let prompt = InputPrompt { input: "", cursor_index: 0, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let lines = prompt.render(&ctx);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn top_rule_is_horizontal_line() {
        let prompt = InputPrompt { input: "", cursor_index: 0, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let lines = prompt.render(&ctx);
        assert!(lines[0].plain_text().chars().all(|c| c == '─'));
        assert_eq!(lines[0].display_width(), 80);
    }

    #[test]
    fn bottom_rule_is_horizontal_line() {
        let prompt = InputPrompt { input: "", cursor_index: 0, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let lines = prompt.render(&ctx);
        assert!(lines[2].plain_text().chars().all(|c| c == '─'));
    }

    #[test]
    fn middle_line_contains_prompt() {
        let prompt = InputPrompt { input: "", cursor_index: 0, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let lines = prompt.render(&ctx);
        assert!(lines[1].plain_text().starts_with("> "));
    }

    #[test]
    fn renders_input_text() {
        let prompt = InputPrompt { input: "hello", cursor_index: 5, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let lines = prompt.render(&ctx);
        assert!(lines[1].plain_text().contains("hello"));
    }

    #[test]
    fn renders_consistently() {
        let prompt = InputPrompt { input: "test", cursor_index: 4, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let a = prompt.render(&ctx);
        let b = prompt.render(&ctx);
        assert_eq!(a, b);
    }

    #[test]
    fn adapts_to_terminal_width() {
        let prompt = InputPrompt { input: "", cursor_index: 0, highlight_range: None };
        let narrow = ViewContext::new((40, 24));
        let wide = ViewContext::new((120, 24));
        let narrow_lines = prompt.render(&narrow);
        let wide_lines = prompt.render(&wide);
        // Both should produce 3 lines but different widths
        assert_eq!(narrow_lines.len(), 3);
        assert_eq!(wide_lines.len(), 3);
        // Wide border should be longer than narrow
        assert!(wide_lines[0].plain_text().len() > narrow_lines[0].plain_text().len());
    }

    #[test]
    fn wraps_long_input() {
        let prompt = InputPrompt {
            input: "this is a very long input that should wrap",
            cursor_index: 41,
            highlight_range: None,
        };
        let ctx = ViewContext::new((20, 24));
        let lines = prompt.render(&ctx);
        assert!(lines.len() > 3);
    }

    #[test]
    fn hard_newline_renders_continuation_row() {
        let prompt = InputPrompt { input: "one\ntwo", cursor_index: "one\ntwo".len(), highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let layout = prompt.layout(&ctx);
        let line_text = layout.lines.iter().map(Line::plain_text).collect::<Vec<_>>();
        assert_eq!(line_text, vec!["─".repeat(80), "> one".to_owned(), "  two".to_owned(), "─".repeat(80)]);
        assert_eq!((layout.cursor_row, layout.cursor_col), (2, 5));
    }

    #[test]
    fn cursor_after_hard_newline_starts_continuation_row() {
        let prompt = InputPrompt { input: "one\ntwo", cursor_index: 4, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let layout = prompt.layout(&ctx);
        assert_eq!((layout.cursor_row, layout.cursor_col), (2, 2));
    }

    #[test]
    fn mention_and_plain_text_both_render() {
        let prompt = InputPrompt { input: "@main.rs explain this", cursor_index: 20, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let lines = prompt.render(&ctx);
        assert!(lines[1].plain_text().contains("@main.rs"));
        assert!(lines[1].plain_text().contains("explain this"));
    }

    #[test]
    fn hard_newline_terminates_mention_styling() {
        let prompt = InputPrompt { input: "@main.rs\nhello", cursor_index: 14, highlight_range: None };
        let ctx = ViewContext::new((80, 24));
        let layout = prompt.layout(&ctx);

        let styled_spans = layout
            .lines
            .iter()
            .map(|line| line.spans().iter().map(|span| (span.text().to_owned(), span.style().fg)).collect::<Vec<_>>())
            .collect::<Vec<_>>();

        assert_eq!(
            styled_spans,
            vec![
                vec![("─".repeat(80), Some(ctx.theme.muted()))],
                vec![("> ".to_owned(), Some(ctx.theme.primary())), ("@main.rs".to_owned(), Some(ctx.theme.info()))],
                vec![("  ".to_owned(), Some(ctx.theme.muted())), ("hello".to_owned(), Some(ctx.theme.text_primary()))],
                vec![("─".repeat(80), Some(ctx.theme.muted()))],
            ]
        );
    }
}
