use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{AnyElement, App, ClickEvent, Window, div, prelude::*};

/// The agent's reasoning, folded into one line that says whether it is still thinking. Clicking
/// it shows or hides the text.
///
/// The line is tagged `thinking-<index>` and the text `thinking-text-<index>`.
pub(crate) fn thinking_block(
    text: String,
    streaming: bool,
    index: usize,
    expanded: bool,
    theme: &UiTheme,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let label = if streaming { "Thinking…" } else { "Thought" };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .text_sm()
        .text_color(theme.colors.text_muted)
        .child(
            div()
                .id(("thinking", index))
                .debug_selector(move || format!("thinking-{index}"))
                .flex()
                .gap_2()
                .cursor_pointer()
                .hover(|line| line.text_color(theme.colors.text_secondary))
                .on_click(on_toggle)
                .child(if expanded { "▾" } else { "▸" })
                .child(label),
        )
        .when(expanded, |block| {
            block.child(
                div()
                    .debug_selector(move || format!("thinking-text-{index}"))
                    .pl_3()
                    .border_l_2()
                    .border_color(theme.colors.border)
                    .child(text),
            )
        })
        .into_any_element()
}
