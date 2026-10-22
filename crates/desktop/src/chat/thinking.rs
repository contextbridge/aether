use crate::ui::UiTheme;
use crate::ui::tokens::ICON_SIZE_SMALL;
use crate::ui::{Icon, IconName, Typography, TypographyExt};
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
    let colors = theme.colors;
    let label = if streaming { "Thinking…" } else { "Thought" };
    let chevron = if expanded { IconName::ChevronDown } else { IconName::ChevronRight };
    let group = format!("thinking-{index}-hover");
    div()
        .flex()
        .flex_col()
        .gap_2()
        .typography(Typography::Small)
        .text_color(colors.text_muted)
        .child(
            div()
                .id(("thinking", index))
                .debug_selector(move || format!("thinking-{index}"))
                .group(group.clone())
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .hover(move |line| line.text_color(colors.text_secondary))
                .on_click(on_toggle)
                .child(
                    Icon::new(chevron, colors.text_muted)
                        .size(ICON_SIZE_SMALL)
                        .group_hover(group, colors.text_secondary),
                )
                .child(label),
        )
        .when(expanded, |block| {
            block.child(
                div()
                    .debug_selector(move || format!("thinking-text-{index}"))
                    .pl_3()
                    .border_l_2()
                    .border_color(colors.border)
                    .child(text),
            )
        })
        .into_any_element()
}
