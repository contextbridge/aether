use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::wrap::wrap_lines;
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

struct MarkdownRenderer<'a> {
    theme: &'a Theme,
    highlighter: &'a mut SyntaxHighlighter,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    code: String,
    code_language: String,
    width: u16,
    in_code_block: bool,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(theme: &'a Theme, highlighter: &'a mut SyntaxHighlighter, width: u16) -> Self {
        Self {
            theme,
            highlighter,
            lines: Vec::new(),
            current: Vec::new(),
            styles: Vec::new(),
            lists: Vec::new(),
            quote_depth: 0,
            code: String::new(),
            code_language: String::new(),
            width,
            in_code_block: false,
        }
    }

    fn render(mut self, source: &str) -> Vec<Line<'static>> {
        let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
        for event in Parser::new_ext(source, options) {
            self.on_event(event);
        }
        self.flush_current();
        while self.lines.last().is_some_and(line_is_empty) {
            self.lines.pop();
        }
        wrap_lines(self.lines, self.width)
    }

    fn on_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.on_start(tag),
            Event::End(tag) => self.on_end(tag),
            Event::Text(text) if self.in_code_block => self.code.push_str(&text),
            Event::Text(text) => self.push_text(text.into_string()),
            Event::Code(code) => {
                self.push_quote_prefix();
                self.current.push(Span::styled(
                    code.into_string(),
                    self.current_style().patch(Style::new().fg(self.theme.code_fg).bg(self.theme.code_bg)),
                ));
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
            Tag::Heading { level, .. } => {
                self.flush_current();
                self.styles.push(Style::new().fg(self.theme.heading).add_modifier(Modifier::BOLD));
                self.push_text(format!("{} ", "#".repeat(level as usize)));
            }
            Tag::Strong => self.styles.push(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Emphasis => self.styles.push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strikethrough => self.styles.push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } => {
                self.styles.push(Style::new().fg(self.theme.link).add_modifier(Modifier::UNDERLINED));
            }
            Tag::BlockQuote(_) => {
                self.flush_current();
                self.quote_depth += 1;
                self.styles.push(Style::new().fg(self.theme.blockquote).add_modifier(Modifier::ITALIC));
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
                self.current.push(Span::styled(format!("{indent}{marker}"), Style::new().fg(self.theme.muted)));
            }
            Tag::CodeBlock(kind) => {
                self.flush_current();
                self.in_code_block = true;
                self.code.clear();
                self.code_language = match kind {
                    CodeBlockKind::Fenced(language) => language.split(',').next().unwrap_or_default().to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
            }
            Tag::Table(_) | Tag::TableHead | Tag::TableRow => self.flush_current(),
            Tag::TableCell if !self.current.is_empty() => self.push_text(" | ".to_string()),
            _ => {}
        }
    }

    fn on_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.flush_current();
                self.push_blank();
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => {
                self.styles.pop();
            }
            TagEnd::Paragraph => {
                self.flush_current();
                self.push_blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current();
                self.styles.pop();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.push_blank();
            }
            TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow => self.flush_current(),
            TagEnd::List(_) => {
                self.lists.pop();
                self.push_blank();
            }
            TagEnd::CodeBlock => self.finish_code_block(),
            TagEnd::Table => self.push_blank(),
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
        self.push_quote_prefix();
        let style = Style::new().fg(self.theme.text_primary).patch(self.current_style());
        self.current.push(Span::styled(text, style));
    }

    fn push_quote_prefix(&mut self) {
        if self.current.is_empty() && self.quote_depth > 0 {
            self.current.push(Span::styled("  ".repeat(self.quote_depth), Style::new().fg(self.theme.blockquote)));
        }
    }

    fn current_style(&self) -> Style {
        self.styles.iter().copied().fold(Style::default(), Style::patch)
    }

    fn flush_current(&mut self) {
        if !self.current.is_empty() {
            self.lines.push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    fn push_blank(&mut self) {
        if !self.lines.is_empty() && !self.lines.last().is_some_and(line_is_empty) {
            self.lines.push(Line::default());
        }
    }
}

fn line_is_empty(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.is_empty())
}
