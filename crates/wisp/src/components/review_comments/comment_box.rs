use tui::{Color, Component, Frame, Line, Style, ViewContext};

pub(super) const INDENT: usize = 2;
pub(super) const BOX_LEFT: &str = "│ ";
pub(super) const BOX_LEFT_WIDTH: usize = 2;
pub(super) const DRAFT_TEXT_COL_START: usize = INDENT + BOX_LEFT_WIDTH;

pub(crate) struct CommentBox<'a> {
    pub text: &'a str,
}

impl Component for CommentBox<'_> {
    type Message = ();

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let width = usize::from(ctx.size.width);
        let bg = ctx.theme.sidebar_bg();
        let border_fg = ctx.theme.accent();
        let text_fg = ctx.theme.text_primary();
        let dashes = width.saturating_sub(INDENT + 1);
        let inner_width = width.saturating_sub(DRAFT_TEXT_COL_START + 1);
        let wrapped = wrap_text(self.text, inner_width);
        let mut rows = Vec::new();
        push_border_row(&mut rows, "┌", dashes, width, border_fg, bg);

        for text_line in &wrapped {
            let mut row = Line::default();
            row.push_with_style(" ".repeat(INDENT), Style::default().bg_color(bg));
            row.push_with_style(BOX_LEFT, Style::fg(border_fg).bg_color(bg));
            row.push_with_style(text_line.as_str(), Style::fg(text_fg).bg_color(bg));
            row.extend_bg_to_width(width);
            rows.push(row);
        }

        push_border_row(&mut rows, "└", dashes, width, border_fg, bg);
        Frame::new(rows)
    }
}

pub(super) fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if current_len == 0 {
            current.push_str(word);
            current_len = word_len;
        } else if current_len + 1 + word_len <= max_width {
            current.push(' ');
            current.push_str(word);
            current_len += 1 + word_len;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_len = word_len;
        }
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    lines
}

fn push_border_row(rows: &mut Vec<Line>, corner: &str, dashes: usize, width: usize, border_fg: Color, bg: Color) {
    let mut row = Line::default();
    row.push_with_style(" ".repeat(INDENT), Style::default().bg_color(bg));
    row.push_with_style(corner, Style::fg(border_fg).bg_color(bg));
    row.push_with_style("─".repeat(dashes), Style::fg(border_fg).bg_color(bg));
    row.extend_bg_to_width(width);
    rows.push(row);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_basic() {
        let result = wrap_text("hello world foo bar", 10);
        assert_eq!(result, vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn component_render_uses_context_width() {
        let mut comment_box = CommentBox { text: "hello world" };
        let ctx = ViewContext::new((20, 4));

        let frame = comment_box.render(&ctx);

        assert_eq!(frame.lines().len(), 3);
        assert!(frame.lines()[1].plain_text().contains("hello world"));
    }
}
