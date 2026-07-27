use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::wrap::wrap_line;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render_markdown(
    source: &str,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    MarkdownRenderer::new(theme, highlighter, width).render(source)
}

/// A fenced code-block delimiter: the character it is drawn with, how many of
/// them, and whatever follows on the line.
///
/// The run length is part of it because a fence only closes on a run at least
/// as long as the one that opened it, so a four-backtick block can contain
/// three-backtick fences of its own without ending early.
pub(crate) struct Fence<'a> {
    character: char,
    length: usize,
    rest: &'a str,
}

impl<'a> Fence<'a> {
    /// The fence delimiter `line` is, when it is one.
    pub(crate) fn parse(line: &'a str) -> Option<Self> {
        let trimmed = line.trim_start();
        let character = trimmed.chars().next().filter(|&c| c == '`' || c == '~')?;
        let length = trimmed.chars().take_while(|&c| c == character).count();
        (length >= 3).then(|| Self { character, length, rest: &trimmed[length..] })
    }

    /// The language tag on an opening fence.
    pub(crate) fn language(&self) -> &'a str {
        self.rest.split_whitespace().next().unwrap_or("")
    }

    /// Whether this delimiter closes a block that `opening` started.
    pub(crate) fn closes(&self, opening: &Self) -> bool {
        self.character == opening.character && self.length >= opening.length && self.rest.trim().is_empty()
    }
}

struct MarkdownRenderer<'a> {
    theme: &'a Theme,
    highlighter: &'a mut SyntaxHighlighter,
    lines: Vec<Line<'static>>,
    inline: InlineMarkdownSpanBuilder<'a>,
    lists: Vec<Option<u64>>,
    code: String,
    code_language: String,
    width: u16,
    in_code_block: bool,
    table_rows: Vec<Vec<Vec<Span<'static>>>>,
    in_table: bool,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(theme: &'a Theme, highlighter: &'a mut SyntaxHighlighter, width: u16) -> Self {
        Self {
            theme,
            highlighter,
            lines: Vec::new(),
            inline: InlineMarkdownSpanBuilder::new(theme),
            lists: Vec::new(),
            code: String::new(),
            code_language: String::new(),
            width,
            in_code_block: false,
            table_rows: Vec::new(),
            in_table: false,
        }
    }

    fn render(mut self, source: &str) -> Vec<Line<'static>> {
        let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
        for event in Parser::new_ext(source, options) {
            self.on_event(event);
        }
        self.flush_current();
        if !source_ends_with_blank_line(source) {
            while self.lines.last().is_some_and(line_is_empty) {
                self.lines.pop();
            }
        }
        self.lines.into_iter().flat_map(|line| wrap_line(line, self.width)).collect()
    }

    fn on_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.on_start(tag),
            Event::End(tag) => self.on_end(tag),
            Event::Text(text) if self.in_code_block => self.code.push_str(&text),
            Event::Text(text) => self.push_text(text.into_string()),
            Event::Code(code) => {
                self.inline.push_quote_prefix();
                self.inline.push_code(code.into_string());
            }
            Event::SoftBreak => self.push_text(" ".to_string()),
            Event::HardBreak => self.flush_current(),
            Event::Rule => {
                self.flush_current();
                self.lines
                    .push(Line::styled("─".repeat(usize::from(self.width.max(1))), Style::new().fg(self.theme.muted)));
                self.push_blank();
            }
            _ => {}
        }
    }

    fn on_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { .. } | Tag::BlockQuote(_) => {
                self.flush_current();
                self.inline.start(&tag);
            }
            Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. } => {
                self.inline.start(&tag);
            }
            Tag::List(start) => {
                self.flush_current();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush_current();
                let indent = "  ".repeat(self.lists.len().saturating_sub(1));
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "• ".to_string(),
                };
                self.inline.push_span(Span::styled(format!("{indent}{marker}"), Style::new().fg(self.theme.muted)));
            }
            Tag::CodeBlock(kind) => {
                self.flush_current();
                self.in_code_block = true;
                self.code.clear();
                self.code_language = match kind {
                    CodeBlockKind::Fenced(language) => language.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
            }
            Tag::Table(_) => {
                self.flush_current();
                self.in_table = true;
                self.table_rows.clear();
            }
            Tag::TableHead | Tag::TableRow => {
                self.table_rows.push(Vec::new());
            }
            _ => {}
        }
    }

    fn on_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.inline.end(tag);
                self.flush_current();
                self.push_blank();
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => {
                self.inline.end(tag);
            }
            TagEnd::Paragraph => {
                self.flush_current();
                self.push_blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current();
                self.inline.end(tag);
                self.push_blank();
            }
            TagEnd::Item => self.flush_current(),
            TagEnd::TableCell => {
                if self.in_table {
                    let cell_spans = self.inline.take_spans();
                    if let Some(row) = self.table_rows.last_mut() {
                        row.push(cell_spans);
                    }
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.push_blank();
            }
            TagEnd::CodeBlock => self.finish_code_block(),
            TagEnd::Table => {
                self.in_table = false;
                self.render_table();
                self.push_blank();
            }
            _ => {}
        }
    }

    fn finish_code_block(&mut self) {
        let mut lines = self.highlighter.highlight(&self.code, &self.code_language, self.theme);
        for line in &mut lines {
            line.style = line.style.patch(Style::new().bg(self.theme.code_bg));
        }
        self.lines.extend(lines);
        self.in_code_block = false;
        self.code.clear();
        self.code_language.clear();
        self.push_blank();
    }

    fn push_text(&mut self, text: String) {
        self.inline.push_text(text);
    }

    fn flush_current(&mut self) {
        if !self.inline.is_empty() {
            self.lines.push(Line::from(self.inline.take_spans()));
        }
    }

    fn push_blank(&mut self) {
        if !self.lines.is_empty() && !self.lines.last().is_some_and(line_is_empty) {
            self.lines.push(Line::default());
        }
    }

    fn render_table(&mut self) {
        let num_columns = self.table_rows.iter().map(Vec::len).max().unwrap_or(0);
        if num_columns == 0 {
            self.table_rows.clear();
            return;
        }

        let column_widths = compute_column_widths(&self.table_rows, num_columns);
        let border_style = Style::new().fg(self.theme.muted);
        let header_style = Style::new().fg(self.theme.heading).add_modifier(Modifier::BOLD);
        let body_style = Style::new().fg(self.theme.text_primary);

        for (row_index, row) in self.table_rows.drain(..).enumerate() {
            let row_style = if row_index == 0 { header_style } else { body_style };
            self.lines.push(render_table_row(&row, &column_widths, row_style, border_style));
            if row_index == 0 {
                let dashes = column_widths.iter().map(|&w| "-".repeat(w + 2)).collect::<Vec<_>>().join("|");
                self.lines.push(Line::styled(format!("|{dashes}|"), border_style));
            }
        }
    }
}

pub(crate) struct InlineMarkdownSpanBuilder<'a> {
    theme: &'a Theme,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    quote_depth: usize,
}

impl<'a> InlineMarkdownSpanBuilder<'a> {
    pub(crate) fn new(theme: &'a Theme) -> Self {
        Self { theme, spans: Vec::new(), styles: Vec::new(), quote_depth: 0 }
    }

    pub(crate) fn start(&mut self, tag: &Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                let style = Style::new().fg(self.theme.heading).add_modifier(Modifier::BOLD);
                self.styles.push(style);
                self.spans.push(Span::styled(format!("{} ", "#".repeat(*level as usize)), style));
            }
            Tag::Strong => self.styles.push(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Emphasis => self.styles.push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strikethrough => self.styles.push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } => self.styles.push(Style::new().fg(self.theme.link).add_modifier(Modifier::UNDERLINED)),
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
                self.styles.push(Style::new().fg(self.theme.blockquote).add_modifier(Modifier::ITALIC));
            }
            _ => self.styles.push(Style::default()),
        }
    }

    pub(crate) fn end(&mut self, tag: TagEnd) {
        self.styles.pop();
        if matches!(tag, TagEnd::BlockQuote(_)) {
            self.quote_depth = self.quote_depth.saturating_sub(1);
        }
    }

    pub(crate) fn push_text(&mut self, text: String) {
        self.push_quote_prefix();
        self.spans.push(Span::styled(text, Style::new().fg(self.theme.text_primary).patch(self.current_style())));
    }

    pub(crate) fn push_code(&mut self, code: String) {
        self.spans.push(Span::styled(
            code,
            self.current_style().patch(Style::new().fg(self.theme.code_fg).bg(self.theme.code_bg)),
        ));
    }

    pub(crate) fn push_soft_break(&mut self) {
        self.spans.push(Span::raw(" "));
    }

    pub(crate) fn push_quote_prefix(&mut self) {
        if self.spans.is_empty() && self.quote_depth > 0 {
            self.spans.push(Span::styled("  ".repeat(self.quote_depth), Style::new().fg(self.theme.blockquote)));
        }
    }

    pub(crate) fn push_span(&mut self, span: Span<'static>) {
        self.spans.push(span);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub(crate) fn take_spans(&mut self) -> Vec<Span<'static>> {
        std::mem::take(&mut self.spans)
    }

    fn current_style(&self) -> Style {
        self.styles.iter().copied().fold(Style::default(), Style::patch)
    }
}

fn line_is_empty(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.is_empty())
}

fn source_ends_with_blank_line(source: &str) -> bool {
    source.ends_with('\n') && source.lines().next_back().is_some_and(|line| line.trim().is_empty())
}

fn compute_column_widths(rows: &[Vec<Vec<Span<'static>>>], num_columns: usize) -> Vec<usize> {
    let mut widths = vec![0usize; num_columns];
    for row in rows {
        for (col, cell) in row.iter().enumerate() {
            let cell_width: usize = cell.iter().map(Span::width).sum();
            widths[col] = widths[col].max(cell_width);
        }
    }
    widths
}

fn render_table_row(
    row: &[Vec<Span<'static>>],
    column_widths: &[usize],
    row_style: Style,
    border_style: Style,
) -> Line<'static> {
    let mut spans = vec![Span::styled("| ", border_style)];
    for (col, &col_width) in column_widths.iter().enumerate() {
        if col > 0 {
            spans.push(Span::styled(" | ", border_style));
        }
        if let Some(cell) = row.get(col) {
            let cell_width: usize = cell.iter().map(Span::width).sum();
            for span in cell {
                spans.push(Span::styled(span.content.clone(), span.style.patch(row_style)));
            }
            let padding = col_width.saturating_sub(cell_width);
            if padding > 0 {
                spans.push(Span::styled(" ".repeat(padding), row_style));
            }
        } else {
            spans.push(Span::styled(" ".repeat(col_width), row_style));
        }
    }
    spans.push(Span::styled(" |", border_style));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::render_markdown;
    use crate::syntax::SyntaxHighlighter;
    use crate::theme::Theme;
    use ratatui::style::Modifier;

    #[test]
    fn renders_inline_markdown_with_nested_styles() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let lines = render_markdown("**bold and *italic*** with `code`", 80, &theme, &mut highlighter);

        let spans = &lines[0].spans;
        assert_eq!(spans.iter().map(|span| span.content.as_ref()).collect::<String>(), "bold and italic with code");
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD | Modifier::ITALIC));
        assert_eq!(spans.last().unwrap().style.fg, Some(theme.code_fg));
        assert_eq!(spans.last().unwrap().style.bg, Some(theme.code_bg));
    }

    #[test]
    fn renders_table_with_aligned_columns_and_separator() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let source = "\
| Name | Age |
|------|-----|
| Alice | 30 |
| Bob | 25 |
";
        let lines = render_markdown(source, 80, &theme, &mut highlighter);

        let table_lines: Vec<_> = lines.iter().filter(|line| line.width() > 0).collect();
        assert!(table_lines.len() >= 4, "expected header + separator + 2 rows, got {} lines", table_lines.len());

        let header: String = table_lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("Name"));
        assert!(header.contains("Age"));

        let separator: String = table_lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            separator.chars().all(|c| c == '|' || c == '-'),
            "separator should only contain | and -, got: {separator}"
        );

        let row1: String = table_lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        let row2: String = table_lines[3].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(row1.contains("Alice"));
        assert!(row2.contains("Bob"));

        let widths: Vec<usize> = table_lines.iter().map(|l| l.width()).collect();
        assert!(widths.iter().all(|&w| w == widths[0]), "columns misaligned, widths: {widths:?}");
    }

    #[test]
    fn table_preserves_inline_code_styling() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let source = "\
| Type | Example |
|------|---------|
| text | `code` |
";
        let lines = render_markdown(source, 80, &theme, &mut highlighter);

        let table_lines: Vec<_> = lines.iter().filter(|line| line.width() > 0).collect();
        assert!(table_lines.len() >= 3, "expected header + separator + 1 row");

        let body: String = table_lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(body.contains("code"));

        let has_code_style = table_lines[2].spans.iter().any(|s| s.style.fg == Some(theme.code_fg));
        assert!(has_code_style, "code span should retain code_fg color");
    }
}
