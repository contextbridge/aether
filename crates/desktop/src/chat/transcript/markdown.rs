use super::code::{SharedHighlighter, highlight};
use crate::ui::tokens::{RADIUS_CONTROL, rem};
use crate::ui::{Button, ButtonVariant, CODE_FONT, IconName, Typography, TypographyExt, UiTheme, color};
use clankerdiff_syntax::LanguageHint;
use clankerdiff_ui_core::ReviewTheme;
use gpui::{App, ClipboardItem, Div, ElementId, FontWeight, HighlightStyle, StyleRefinement, div, prelude::*};
use gpui_base::text::CodeBlock;
use gpui_base::{TextViewDefaults, TextViewStyle};

pub(crate) fn install(review: &ReviewTheme, ui: &UiTheme, cx: &mut App) {
    let palette = &review.markdown;
    let heading = color(palette.heading);
    let style = TextViewStyle::default()
        .with_foreground(ui.colors.text)
        .with_muted_foreground(ui.colors.text_secondary)
        .with_link(color(palette.link))
        .with_selection(ui.editor_style().selection)
        .with_code_background(color(palette.code_background))
        .with_border(ui.colors.border_subtle)
        .with_paragraph_gap(rem(12.0))
        .with_heading(move |level| {
            let heading = div().text_color(heading);
            refinement(match level {
                1 => heading.typography(Typography::Display),
                2 => heading.typography(Typography::Heading),
                _ => heading.typography(Typography::Reading).font_weight(FontWeight::SEMIBOLD),
            })
        })
        .with_code_block(refinement(
            div()
                .font_family(CODE_FONT)
                .typography(Typography::Small)
                .text_color(color(palette.code))
                .rounded(rem(RADIUS_CONTROL))
                .border_1()
                .border_color(ui.colors.border_subtle),
        ))
        .with_inline_code(HighlightStyle {
            color: Some(color(palette.inline_code)),
            background_color: Some(color(palette.inline_code_background)),
            ..HighlightStyle::default()
        })
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
