use clankerdiff_gpui::style::{color, highlight_style};
use clankerdiff_gpui::ui::prelude::{Button, ControlSize, UiTheme};
use clankerdiff_markdown::{
    MarkdownBlock, MarkdownBlockKind, MarkdownCodeBlock, MarkdownDocument, MarkdownInline, MarkdownListItem,
    MarkdownStream, MarkdownTable,
};
use clankerdiff_syntax::{Fingerprint, LanguageHint, SyntaxHighlighter};
use clankerdiff_theme::{MarkdownPalette, ReviewTheme};
use gpui::{
    AnyElement, ClipboardItem, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString,
    StrikethroughStyle, StyledText, UnderlineStyle, div, prelude::*, px,
};
use std::ops::Range;

/// An assistant message's Markdown, parsed incrementally as its text streams in.
#[derive(Default)]
pub(crate) struct StreamedMarkdown {
    stream: MarkdownStream,
}

/// Paints Markdown documents in the app theme, with syntax-highlighted code blocks that each
/// have a copy button.
pub(crate) struct MarkdownRenderer<'a> {
    review: &'a ReviewTheme,
    ui: &'a UiTheme,
    highlighter: &'a mut SyntaxHighlighter,
    /// Tells this document's interactive elements apart from those of other documents.
    scope: SharedString,
    code_blocks: u64,
    texts: u64,
}

impl StreamedMarkdown {
    /// Catches the parse up with `text`, the message's whole text so far. A `finished` message
    /// can no longer change, which settles its last block.
    pub(crate) fn sync(&mut self, text: &str, finished: bool) {
        match text.strip_prefix(self.stream.source()) {
            Some("") => {}
            Some(appended) => {
                self.stream.push(appended);
            }
            None => {
                self.stream.replace(text);
            }
        }
        if finished && !self.stream.is_finished() {
            self.stream.finish();
        }
    }

    pub(crate) fn document(&self) -> &MarkdownDocument {
        self.stream.document()
    }
}

impl<'a> MarkdownRenderer<'a> {
    /// A renderer whose copy buttons are tagged `copy-code-<scope>-<n>`, counting code blocks
    /// in reading order.
    pub(crate) fn new(
        scope: impl Into<SharedString>,
        review: &'a ReviewTheme,
        ui: &'a UiTheme,
        highlighter: &'a mut SyntaxHighlighter,
    ) -> Self {
        Self { review, ui, highlighter, scope: scope.into(), code_blocks: 0, texts: 0 }
    }

    pub(crate) fn render(mut self, document: &MarkdownDocument) -> AnyElement {
        self.blocks(document.blocks()).into_any_element()
    }

    fn blocks(&mut self, blocks: &[MarkdownBlock]) -> gpui::Div {
        let mut column = div().flex().flex_col().gap_2().min_w_0();
        for block in blocks {
            column = column.child(self.block(block));
        }
        column
    }

    fn block(&mut self, block: &MarkdownBlock) -> AnyElement {
        let palette = &self.review.markdown;
        match &block.kind {
            MarkdownBlockKind::Heading { level, content } => {
                let heading = div().font_weight(FontWeight::SEMIBOLD).text_color(color(palette.heading));
                let heading = match level {
                    1 => heading.text_xl(),
                    2 => heading.text_lg(),
                    _ => heading,
                };
                heading.child(self.inline(content)).into_any_element()
            }
            MarkdownBlockKind::Paragraph { content } => div().child(self.inline(content)).into_any_element(),
            MarkdownBlockKind::List { ordered, start, items } => self.list(*ordered, start.unwrap_or(1), items),
            MarkdownBlockKind::BlockQuote { blocks } => div()
                .pl_3()
                .border_l_2()
                .border_color(color(palette.quote))
                .text_color(self.ui.colors.text_secondary)
                .child(self.blocks(blocks))
                .into_any_element(),
            MarkdownBlockKind::CodeBlock(code) => self.code_block(code),
            MarkdownBlockKind::Table(table) => self.table(table),
            MarkdownBlockKind::HtmlFallback { content } => {
                div().text_color(self.ui.colors.text_muted).child(self.inline(content)).into_any_element()
            }
            MarkdownBlockKind::Rule => div().h(px(1.0)).w_full().bg(self.ui.colors.border).into_any_element(),
        }
    }

    fn list(&mut self, ordered: bool, start: u64, items: &[MarkdownListItem]) -> AnyElement {
        let mut list = div().flex().flex_col().gap_1();
        for (number, item) in (start..).zip(items) {
            let marker = if ordered { format!("{number}.") } else { "•".to_owned() };
            let mut body = div().flex_1().min_w_0().flex().flex_col().gap_1();
            if !item.content.is_empty() {
                body = body.child(self.inline(&item.content));
            }
            if !item.blocks.is_empty() {
                body = body.child(self.blocks(&item.blocks));
            }
            list = list.child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_none().text_color(self.ui.colors.text_muted).child(marker))
                    .child(body),
            );
        }
        list.into_any_element()
    }

    fn code_block(&mut self, block: &MarkdownCodeBlock) -> AnyElement {
        let palette = &self.review.markdown;
        let code = block.lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n");
        let highlights = self.highlight(&code, block.highlight_hint());
        let id = ElementId::NamedInteger(format!("copy-code-{}", self.scope).into(), self.code_blocks);
        self.code_blocks += 1;
        let copied = code.clone();
        let copy = Button::new(id, "Copy", *self.ui)
            .size(ControlSize::Small)
            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone())));
        let language = block.language.clone().unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(self.ui.colors.border)
            .bg(color(palette.code_background))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(self.ui.colors.border)
                    .text_xs()
                    .text_color(self.ui.colors.text_muted)
                    .child(language)
                    .child(copy),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(color(palette.code))
                    .child(StyledText::new(code).with_highlights(highlights)),
            )
            .into_any_element()
    }

    fn table(&mut self, table: &MarkdownTable) -> AnyElement {
        let mut grid = div().flex().flex_col().border_1().border_color(self.ui.colors.border).rounded_md();
        for (index, row) in table.rows.iter().enumerate() {
            let mut cells =
                div().flex().when(index > 0, |cells| cells.border_t_1().border_color(self.ui.colors.border));
            for cell in &row.cells {
                cells = cells.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .when(row.header, |cell| cell.font_weight(FontWeight::SEMIBOLD))
                        .child(self.inline(&cell.content)),
                );
            }
            grid = grid.child(cells);
        }
        grid.into_any_element()
    }

    /// Lays out inline Markdown as one run of styled text, whose links open in the browser.
    fn inline(&mut self, inlines: &[MarkdownInline]) -> AnyElement {
        let mut text = InlineText::default();
        text.push_all(inlines, HighlightStyle::default(), &self.review.markdown);
        let styled = StyledText::new(text.text).with_highlights(text.highlights);
        if text.links.is_empty() {
            return styled.into_any_element();
        }
        let id = ElementId::NamedInteger(format!("markdown-text-{}", self.scope).into(), self.texts);
        self.texts += 1;
        let (ranges, urls): (Vec<_>, Vec<_>) = text.links.into_iter().unzip();
        InteractiveText::new(id, styled)
            .on_click(ranges, move |index, _, cx| cx.open_url(&urls[index]))
            .into_any_element()
    }

    fn highlight(&mut self, code: &str, hint: &str) -> Vec<(Range<usize>, HighlightStyle)> {
        let Ok(highlighted) = self.highlighter.with_theme(&self.review.syntax).highlight_document(
            Fingerprint::of([code.as_bytes()]),
            LanguageHint::InfoString(hint),
            || code,
        ) else {
            return Vec::new();
        };
        let mut offset = 0;
        let mut highlights = Vec::new();
        for (index, line) in code.split('\n').enumerate() {
            for span in highlighted.line(index).unwrap_or_default() {
                let range = span.range.start + offset..span.range.end + offset;
                highlights.push((range, highlight_style(span.foreground, span.font_style)));
            }
            offset += line.len() + 1;
        }
        highlights
    }
}

/// Inline Markdown flattened into one string, with the styles and links of its ranges.
#[derive(Default)]
struct InlineText {
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, String)>,
}

impl InlineText {
    fn push_all(&mut self, inlines: &[MarkdownInline], style: HighlightStyle, palette: &MarkdownPalette) {
        for inline in inlines {
            self.push_inline(inline, style, palette);
        }
    }

    fn push_inline(&mut self, inline: &MarkdownInline, style: HighlightStyle, palette: &MarkdownPalette) {
        match inline {
            MarkdownInline::Text(text) | MarkdownInline::ImageAlt(text) => self.push(text, style),
            MarkdownInline::Code(code) => self.push(
                code,
                style.highlight(HighlightStyle {
                    color: Some(color(palette.inline_code)),
                    background_color: Some(color(palette.inline_code_background)),
                    ..HighlightStyle::default()
                }),
            ),
            MarkdownInline::Strong(children) => self.push_all(
                children,
                style.highlight(HighlightStyle { font_weight: Some(FontWeight::BOLD), ..HighlightStyle::default() }),
                palette,
            ),
            MarkdownInline::Emphasis(children) => self.push_all(
                children,
                style.highlight(HighlightStyle { font_style: Some(FontStyle::Italic), ..HighlightStyle::default() }),
                palette,
            ),
            MarkdownInline::Strikethrough(children) => self.push_all(
                children,
                style.highlight(HighlightStyle {
                    strikethrough: Some(StrikethroughStyle { thickness: px(1.0), color: None }),
                    ..HighlightStyle::default()
                }),
                palette,
            ),
            MarkdownInline::Link { destination, content, .. } => {
                let start = self.text.len();
                let link = color(palette.link);
                let style = style.highlight(HighlightStyle {
                    color: Some(link),
                    underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(link), wavy: false }),
                    ..HighlightStyle::default()
                });
                self.push_all(content, style, palette);
                self.links.push((start..self.text.len(), destination.clone()));
            }
            MarkdownInline::SoftBreak => self.push(" ", style),
            MarkdownInline::HardBreak => self.push("\n", style),
        }
    }

    fn push(&mut self, text: &str, style: HighlightStyle) {
        let start = self.text.len();
        self.text.push_str(text);
        if style != HighlightStyle::default() && !text.is_empty() {
            self.highlights.push((start..self.text.len(), style));
        }
    }
}
