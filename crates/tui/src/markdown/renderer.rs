use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Parser, Tag, TagEnd};

use super::headings::MarkdownHeading;
use super::pulldown_options;
use super::source_map::SourceMap;
use super::table::{CellBuilder, TableCell, TableState, line_display_width};
use crate::line::Line;
use crate::rendering::render_context::ViewContext;
use crate::span::Span;
use crate::style::Style;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMappedLine {
    pub source_line_no: usize,
    pub line: Line,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownBlock {
    pub anchor_line_no: usize,
    pub rendered_line_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownRenderResult {
    pub lines: Vec<SourceMappedLine>,
    pub headings: Vec<MarkdownHeading>,
    pub blocks: Vec<MarkdownBlock>,
}

impl MarkdownRenderResult {
    pub fn to_lines(self) -> Vec<Line> {
        self.lines.into_iter().map(|line| line.line).collect()
    }
}

pub fn render_markdown_result(text: &str, context: &ViewContext) -> MarkdownRenderResult {
    let source = SourceMap::new(text);
    MarkdownRenderer::new(context, &source).render()
}

struct MarkdownRenderer<'a> {
    context: &'a ViewContext,
    theme: &'a Theme,
    source: &'a SourceMap<'a>,
    style_stack: InlineStyleStack,
    headings: HeadingCollector,
    display_lines: Vec<Line>,
    display_line_sources: Vec<usize>,
    current_line: Line,
    current_source_line_no: usize,
    list_stack: Vec<Option<u64>>,
    list_item_stack: Vec<ListItemState>,
    code_buffer: String,
    code_lang: String,
    in_code_block: bool,
    blockquote_depth: usize,
    table_state: Option<TableState>,
    active_cell: Option<CellBuilder>,
    blocks: Vec<MarkdownBlock>,
    current_block: Option<BlockBuilder>,
}

struct BlockBuilder {
    anchor_line_no: usize,
    first_rendered_line: usize,
}

#[derive(Default)]
struct ListItemState {
    is_loose: bool,
}

#[derive(Clone, Copy)]
enum BlockSpacing {
    None,
    BlankLineAfter,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(context: &'a ViewContext, source: &'a SourceMap<'a>) -> Self {
        Self {
            context,
            theme: &context.theme,
            source,
            style_stack: InlineStyleStack::new(),
            headings: HeadingCollector::new(),
            display_lines: Vec::new(),
            display_line_sources: Vec::new(),
            current_line: Line::default(),
            current_source_line_no: 1,
            list_stack: Vec::new(),
            list_item_stack: Vec::new(),
            code_buffer: String::new(),
            code_lang: String::new(),
            in_code_block: false,
            blockquote_depth: 0,
            table_state: None,
            active_cell: None,
            blocks: Vec::new(),
            current_block: None,
        }
    }

    fn render(mut self) -> MarkdownRenderResult {
        let parser = Parser::new_ext(self.source.text(), pulldown_options()).into_offset_iter();
        for (event, range) in parser {
            self.handle_event(event, range);
        }

        self.flush_line();
        while self.display_lines.last().is_some_and(Line::is_empty) {
            self.display_lines.pop();
            self.display_line_sources.pop();
        }

        self.finalize_current_block();

        let lines: Vec<SourceMappedLine> = self
            .display_lines
            .into_iter()
            .zip(self.display_line_sources)
            .map(|(line, source_line_no)| SourceMappedLine { source_line_no, line })
            .collect();
        MarkdownRenderResult { lines, headings: self.headings.into_headings(), blocks: self.blocks }
    }

    fn handle_event(&mut self, event: Event<'_>, range: Range<usize>) {
        self.current_source_line_no = self.source.line_no_for_start(&range);

        match event {
            Event::Start(tag) => self.handle_start(tag, range),
            Event::End(tag_end) => self.handle_end(tag_end),
            Event::Text(text) => {
                self.headings.append_text(&text);
                self.push_inline_text(&text);
            }
            Event::Code(code) => {
                self.headings.append_text(&code);
                self.push_inline_code(&code);
            }
            Event::SoftBreak => self.push_soft_break(),
            Event::HardBreak => self.push_hard_break(),
            Event::Rule => {
                self.finish_current_line();
                self.start_block(self.current_source_line_no);
                self.push_line(Line::with_style("───────────────", Style::fg(self.theme.muted())));
                self.finish_rendered_block(BlockSpacing::BlankLineAfter);
            }
            _ => {}
        }
    }

    fn handle_start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.headings.begin(level as u8, self.source.line_no_for_start(&range));
                self.finish_current_line();
                self.start_block(self.current_source_line_no);
                let prefix = "#".repeat(level as usize);
                let style = heading_style(level as u8, self.theme);
                self.push_styled_text(&format!("{prefix} "), style);
                self.style_stack.push(style);
            }
            Tag::Paragraph if self.list_item_stack.is_empty() => {
                self.start_block(self.current_source_line_no);
            }
            Tag::BlockQuote(_) => {
                self.finish_current_line();
                self.blockquote_depth += 1;
                self.style_stack.push(Style::fg(self.theme.blockquote()));
            }
            Tag::List(start) => {
                if self.list_stack.is_empty() {
                    self.finish_current_line();
                }
                self.list_stack.push(start);
            }
            Tag::Item => {
                self.list_item_stack.push(ListItemState::default());
                self.finish_current_line();
                self.start_block(self.current_source_line_no);
                let indent = "  ".repeat(self.list_stack.len().saturating_sub(1));
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let marker = format!("{n}. ");
                        *n += 1;
                        marker
                    }
                    _ => "- ".to_string(),
                };
                self.push_styled_text(&format!("{indent}{marker}"), Style::fg(self.theme.muted()));
            }
            Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. } => {
                self.style_stack.push_inline_tag(&tag, self.theme);
            }
            Tag::CodeBlock(kind) => self.handle_code_block_start(kind),
            Tag::Table(_) | Tag::TableRow | Tag::TableCell => self.handle_table_start(tag),
            _ => {}
        }
    }

    fn handle_end(&mut self, tag_end: TagEnd) {
        match tag_end {
            TagEnd::Paragraph => self.finish_block(BlockSpacing::BlankLineAfter),
            TagEnd::Heading(_) => {
                self.style_stack.pop();
                self.headings.finish();
                self.finish_block(BlockSpacing::BlankLineAfter);
            }
            TagEnd::BlockQuote(_) => {
                self.style_stack.pop();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
                let spacing =
                    if self.blockquote_depth == 0 { BlockSpacing::BlankLineAfter } else { BlockSpacing::None };
                self.finish_block(spacing);
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.finish_block(BlockSpacing::BlankLineAfter);
                }
            }
            TagEnd::Item => {
                self.finish_current_line();
                if self.list_item_stack.pop().is_some_and(|state| state.is_loose) {
                    self.push_blank_line();
                }
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => self.style_stack.pop(),
            TagEnd::CodeBlock => self.handle_code_block_end(),
            TagEnd::Table | TagEnd::TableRow | TagEnd::TableHead | TagEnd::TableCell => self.handle_table_end(tag_end),
            _ => {}
        }
    }

    fn handle_code_block_start(&mut self, kind: CodeBlockKind<'_>) {
        self.finish_current_line();
        self.start_block(self.current_source_line_no);
        self.in_code_block = true;
        self.code_buffer.clear();
        self.code_lang = match kind {
            CodeBlockKind::Fenced(lang) => lang.split(',').next().unwrap_or("").trim().to_string(),
            CodeBlockKind::Indented => String::new(),
        };
    }

    fn handle_code_block_end(&mut self) {
        self.in_code_block = false;
        let code = std::mem::take(&mut self.code_buffer);
        let lang = std::mem::take(&mut self.code_lang);
        let code_lines = self.context.highlighter().highlight(&code, &lang, self.theme);
        self.extend_lines(code_lines);
        self.finish_rendered_block(BlockSpacing::BlankLineAfter);
    }

    fn handle_table_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Table(alignments) => {
                self.finish_current_line();
                self.start_block(self.current_source_line_no);
                self.table_state = Some(TableState::new(&alignments));
            }
            Tag::TableRow => {
                if let Some(ref mut table) = self.table_state {
                    table.start_row();
                }
            }
            Tag::TableCell => {
                self.active_cell = Some(CellBuilder::default());
            }
            _ => {}
        }
    }

    fn handle_table_end(&mut self, tag_end: TagEnd) {
        match tag_end {
            TagEnd::Table => {
                if let Some(table) = self.table_state.take() {
                    let rendered = table.render(self.theme);
                    self.extend_lines(rendered);
                    self.finish_rendered_block(BlockSpacing::BlankLineAfter);
                }
            }
            TagEnd::TableRow | TagEnd::TableHead => {
                if let Some(ref mut table) = self.table_state {
                    table.finish_row();
                }
            }
            TagEnd::TableCell => {
                if let Some(builder) = self.active_cell.take()
                    && let Some(ref mut table) = self.table_state
                {
                    let col_idx = table.current_row.len();
                    let alignment = table.alignments.get(col_idx).copied().unwrap_or(Alignment::None);
                    let lines = builder.finish();
                    let max_width = lines.iter().map(line_display_width).max().unwrap_or(0);
                    let cell = TableCell { lines, alignment, max_width };
                    table.add_cell(cell);
                }
            }
            _ => {}
        }
    }

    fn push_inline_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_buffer.push_str(text);
            return;
        }

        let style = self.style_stack.current();
        if let Some(cell) = self.active_cell.as_mut() {
            cell.push_text(text, style);
            return;
        }

        self.push_text(text);
    }

    fn push_inline_code(&mut self, code: &str) {
        if self.in_code_block {
            self.code_buffer.push_str(code);
            return;
        }

        let style = Style::fg(self.theme.code_fg());
        if let Some(cell) = self.active_cell.as_mut() {
            cell.push_code(code, style);
        } else {
            self.current_line.push_span(Span::with_style(code, style));
        }
    }

    fn push_soft_break(&mut self) {
        if self.in_code_block {
            self.code_buffer.push('\n');
            return;
        }

        let style = self.style_stack.current();
        if let Some(cell) = self.active_cell.as_mut() {
            cell.soft_break(style);
            return;
        }

        self.push_text(" ");
    }

    fn push_hard_break(&mut self) {
        if self.in_code_block {
            self.code_buffer.push('\n');
            return;
        }

        if let Some(cell) = self.active_cell.as_mut() {
            cell.hard_break();
            return;
        }

        self.flush_line();
    }

    fn push_text(&mut self, text: &str) {
        let style = self.style_stack.current();
        let prefix = self.blockquote_prefix();

        for (index, chunk) in text.split('\n').enumerate() {
            if index > 0 {
                self.flush_line();
            }
            if self.current_line.is_empty() && !prefix.is_empty() {
                self.current_line.push_with_style(&*prefix, Style::fg(self.theme.blockquote()));
            }
            if !chunk.is_empty() {
                self.current_line.push_span(Span::with_style(chunk, style));
            }
        }
    }

    fn push_styled_text(&mut self, text: &str, style: Style) {
        self.current_line.push_span(Span::with_style(text, style));
    }

    fn finish_current_line(&mut self) {
        if !self.current_line.is_empty() {
            self.flush_line();
        }
    }

    fn finish_block(&mut self, spacing: BlockSpacing) {
        self.finish_current_line();
        self.finish_rendered_block(spacing);
    }

    fn finish_rendered_block(&mut self, spacing: BlockSpacing) {
        if matches!(spacing, BlockSpacing::BlankLineAfter) {
            if let Some(item_state) = self.list_item_stack.last_mut() {
                item_state.is_loose = true;
            }
            self.push_blank_line();
        }
    }

    fn push_blank_line(&mut self) {
        if self.display_lines.is_empty() {
            return;
        }
        if self.display_lines.last().is_some_and(Line::is_empty) {
            return;
        }
        self.push_line(Line::default());
    }

    fn flush_line(&mut self) {
        let prefix = self.blockquote_prefix();
        if !prefix.is_empty() && self.current_line.is_empty() {
            self.current_line.push_with_style(&*prefix, Style::fg(self.theme.blockquote()));
        }
        let line = std::mem::take(&mut self.current_line);
        self.push_line(line);
    }

    fn push_line(&mut self, line: Line) {
        self.display_lines.push(line);
        self.display_line_sources.push(self.current_source_line_no);
    }

    fn extend_lines(&mut self, lines: Vec<Line>) {
        for line in lines {
            self.push_line(line);
        }
    }

    fn blockquote_prefix(&self) -> Cow<'static, str> {
        if self.blockquote_depth == 0 { Cow::Borrowed("") } else { Cow::Owned("  ".repeat(self.blockquote_depth)) }
    }

    fn start_block(&mut self, anchor_line_no: usize) {
        self.finalize_current_block();
        self.current_block = Some(BlockBuilder { anchor_line_no, first_rendered_line: self.display_lines.len() });
    }

    fn finalize_current_block(&mut self) {
        let Some(builder) = self.current_block.take() else {
            return;
        };
        let mut end = self.display_lines.len();
        while end > builder.first_rendered_line && self.display_lines.get(end - 1).is_some_and(Line::is_empty) {
            end -= 1;
        }
        if end > builder.first_rendered_line {
            self.blocks.push(MarkdownBlock {
                anchor_line_no: builder.anchor_line_no,
                rendered_line_range: builder.first_rendered_line..end,
            });
        }
    }
}

struct InlineStyleStack {
    stack: Vec<Style>,
}

impl InlineStyleStack {
    fn new() -> Self {
        Self { stack: Vec::new() }
    }

    fn push(&mut self, style: Style) {
        self.stack.push(style);
    }

    fn pop(&mut self) {
        self.stack.pop();
    }

    fn current(&self) -> Style {
        self.stack.iter().copied().fold(Style::default(), Style::merge)
    }

    fn push_inline_tag(&mut self, tag: &Tag<'_>, theme: &Theme) {
        match tag {
            Tag::Heading { level, .. } => self.push(heading_style(*level as u8, theme)),
            Tag::BlockQuote(_) => self.push(Style::fg(theme.blockquote())),
            Tag::Strong => self.push(Style::default().bold()),
            Tag::Emphasis => self.push(Style::default().italic()),
            Tag::Strikethrough => self.push(Style::default().strikethrough()),
            Tag::Link { .. } => self.push(Style::fg(theme.link()).underline()),
            _ => {}
        }
    }
}

struct HeadingCollector {
    headings: Vec<MarkdownHeading>,
    active: Option<ActiveHeading>,
}

impl HeadingCollector {
    fn new() -> Self {
        Self { headings: Vec::new(), active: None }
    }

    fn begin(&mut self, level: u8, source_line_no: usize) {
        self.active = Some(ActiveHeading { level, source_line_no, title: String::new() });
    }

    fn append_text(&mut self, text: &str) {
        if let Some(active) = self.active.as_mut() {
            active.title.push_str(text);
        }
    }

    fn finish(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        let title = active.title.trim().to_string();
        if title.is_empty() {
            return;
        }
        self.headings.push(MarkdownHeading { title, level: active.level, source_line_no: active.source_line_no });
    }

    fn into_headings(self) -> Vec<MarkdownHeading> {
        self.headings
    }
}

fn heading_style(level: u8, theme: &Theme) -> Style {
    match level {
        1 | 2 => Style::fg(theme.heading()).bold(),
        _ => Style::fg(theme.text_secondary()).bold(),
    }
}

struct ActiveHeading {
    level: u8,
    source_line_no: usize,
    title: String,
}
