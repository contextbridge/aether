use super::code::{SharedHighlighter, highlight};
use crate::ui::tokens::{RADIUS_CONTAINER, rem};
use crate::ui::{Button, ButtonVariant, CODE_FONT, IconName, Typography, TypographyExt, UiTheme, color};
use clankerdiff_syntax::LanguageHint;
use clankerdiff_theme::ReviewTheme;
use gpui::{App, ClipboardItem, Div, ElementId, HighlightStyle, StyleRefinement, div, prelude::*};
use gpui_base::text::CodeBlock;
use gpui_base::{TextViewDefaults, TextViewStyle};

/// Makes every reply's Markdown paint in `review`, with its code blocks highlighted by the
/// app's shared syntax highlighter.
pub(crate) fn install(review: &ReviewTheme, ui: &UiTheme, cx: &mut App) {
    let palette = &review.markdown;
    let heading = color(palette.heading);
    let style = TextViewStyle::default()
        .with_foreground(ui.colors.text)
        .with_muted_foreground(ui.colors.text_secondary)
        .with_link(color(palette.link))
        .with_selection(ui.editor_style().selection)
        .with_code_background(color(palette.code_background))
        .with_border(ui.colors.border)
        .with_paragraph_gap(rem(8.0))
        .with_heading(move |level| {
            let typography = match level {
                1 => Typography::Display,
                2 => Typography::Heading,
                _ => Typography::Title,
            };
            refinement(div().typography(typography).text_color(heading))
        })
        .with_code_block(refinement(
            div()
                .font_family(CODE_FONT)
                .typography(Typography::Small)
                .text_color(color(palette.code))
                .rounded(rem(RADIUS_CONTAINER))
                .border_1()
                .border_color(ui.colors.border),
        ))
        .with_inline_code(HighlightStyle {
            color: Some(color(palette.inline_code)),
            background_color: Some(color(palette.inline_code_background)),
            ..HighlightStyle::default()
        })
        // Without this, tables paint gpui-base's own light-theme surface behind the app's text.
        .with_table(refinement(div().bg(ui.colors.surface)))
        .with_table_head(refinement(div().typography(Typography::BodyStrong)));
    let syntax = review.syntax.clone();
    let highlighter = SharedHighlighter::global(cx);
    TextViewDefaults::new()
        .with_style(style)
        .with_code_block_highlighter(move |block| {
            let language = block.lang().unwrap_or_default();
            highlighter
                .with(|highlighter| highlight(&block.code(), LanguageHint::InfoString(&language), &syntax, highlighter))
        })
        .install(cx);
}

/// A code block's language and a button that copies its code, tagged `copy-code-<row>-<offset>`
/// by the row's index and the block's byte offset in the reply.
pub(crate) fn code_actions(row: usize, block: &CodeBlock, theme: &UiTheme) -> Div {
    let offset = block.span.map_or(0, |span| span.start);
    let id = ElementId::NamedInteger(format!("copy-code-{row}").into(), offset as u64);
    let code = block.code();
    div()
        .flex()
        .items_center()
        .gap_2()
        .typography(Typography::Caption)
        .text_color(theme.colors.text_muted)
        .children(block.lang())
        .child(
            Button::new(id, "Copy", *theme)
                .icon(IconName::Copy)
                .variant(ButtonVariant::Ghost)
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()))),
        )
}

fn refinement(mut element: Div) -> StyleRefinement {
    element.style().clone()
}
