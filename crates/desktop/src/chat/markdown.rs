use super::code::{highlight, styled_text};
use crate::ui::UiTheme;
use crate::ui::color;
use crate::ui::tokens::{GROUP_HEIGHT, RADIUS_CONTAINER, rem};
use crate::ui::{Button, ButtonVariant, CODE_FONT, IconName, Typography, TypographyExt};
use clankerdiff_markdown::{
    MarkdownBlock, MarkdownBlockKind, MarkdownCodeBlock, MarkdownInline, MarkdownListItem, MarkdownStream,
    MarkdownTable,
};
use clankerdiff_syntax::{LanguageHint, SyntaxHighlighter};
use clankerdiff_theme::{MarkdownPalette, ReviewTheme, ThemeId};
use gpui::{
    AnyElement, ClipboardItem, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString,
    StrikethroughStyle, StyledText, UnderlineStyle, div, prelude::*, px,
};
use std::collections::HashMap;
use std::ops::Range;

/// An assistant message's Markdown, parsed incrementally as its text streams in.
#[derive(Default)]
pub(crate) struct StreamedMarkdown {
    stream: MarkdownStream,
    /// The code blocks as last painted, in the theme they were highlighted in.
    code: CodeCache,
    code_theme: Option<ThemeId>,
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
    /// The code blocks painted last time, and those painted this time.
    previous_code: CodeCache,
    code: CodeCache,
}

/// Highlighted code blocks by where their content sits in the source, with their language hint.
/// The source only grows as it streams, so a range that is still there still holds the same code.
type CodeCache = HashMap<(Range<usize>, String), HighlightedCode>;

#[derive(Clone)]
struct HighlightedCode {
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
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
                self.code.clear();
            }
        }
        if finished && !self.stream.is_finished() {
            self.stream.finish();
        }
    }

    /// Paints the message with `renderer`, highlighting only the code blocks that changed since
    /// it was last painted.
    pub(crate) fn render(&mut self, mut renderer: MarkdownRenderer<'_>) -> AnyElement {
        let theme = renderer.review.id();
        if self.code_theme.as_ref() != Some(theme) {
            self.code.clear();
            self.code_theme = Some(theme.clone());
        }
        renderer.previous_code = std::mem::take(&mut self.code);
        let element = renderer.blocks(self.stream.document().blocks()).into_any_element();
        self.code = renderer.code;
        element
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
        Self {
            review,
            ui,
            highlighter,
            scope: scope.into(),
            code_blocks: 0,
            texts: 0,
            previous_code: CodeCache::new(),
            code: CodeCache::new(),
        }
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
                let typography = match level {
                    1 => Typography::Display,
                    2 => Typography::Heading,
                    _ => Typography::Title,
                };
                div()
                    .when(*level <= 2, Styled::pt_2)
                    .typography(typography)
                    .text_color(color(palette.heading))
                    .child(self.inline(content))
                    .into_any_element()
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
        let key = (block.content.bytes.clone(), block.highlight_hint().to_owned());
        let highlighted = self.previous_code.remove(&key).unwrap_or_else(|| {
            let text = block.lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n");
            let hint = LanguageHint::InfoString(block.highlight_hint());
            let highlights = highlight(&text, hint, &self.review.syntax, self.highlighter);
            HighlightedCode { text: text.into(), highlights }
        });
        self.code.insert(key, highlighted.clone());
        let HighlightedCode { text: code, highlights } = highlighted;
        let id = ElementId::NamedInteger(format!("copy-code-{}", self.scope).into(), self.code_blocks);
        self.code_blocks += 1;
        let copied = code.clone();
        let copy = Button::new(id, "Copy", *self.ui)
            .icon(IconName::Copy)
            .variant(ButtonVariant::Ghost)
            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.to_string())));
        let language = block.language.clone().unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(rem(RADIUS_CONTAINER))
            .border_1()
            .border_color(self.ui.colors.border)
            .bg(color(palette.code_background))
            .child(
                div()
                    .h(rem(GROUP_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl_3()
                    .pr_1()
                    .border_b_1()
                    .border_color(self.ui.colors.border)
                    .typography(Typography::Caption)
                    .text_color(self.ui.colors.text_muted)
                    .child(language)
                    .child(copy),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .font_family(CODE_FONT)
                    .typography(Typography::Small)
                    .text_color(color(palette.code))
                    .child(styled_text(code, highlights)),
            )
            .into_any_element()
    }

    fn table(&mut self, table: &MarkdownTable) -> AnyElement {
        let mut grid = div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_1()
            .border_color(self.ui.colors.border)
            .rounded(rem(RADIUS_CONTAINER));
        for (index, row) in table.rows.iter().enumerate() {
            let mut cells =
                div().flex().when(index > 0, |cells| cells.border_t_1().border_color(self.ui.colors.border));
            for cell in &row.cells {
                cells = cells.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_3()
                        .py_2()
                        .when(row.header, |cell| cell.typography(Typography::BodyStrong).bg(self.ui.colors.surface))
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
