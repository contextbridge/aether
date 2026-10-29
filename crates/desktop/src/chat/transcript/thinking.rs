use crate::ui::UiTheme;
use crate::ui::{Disclosure, Typography, TypographyExt};
use gpui::{AnyElement, App, ClickEvent, Window, div, prelude::*};

pub(crate) fn thinking_block(
    text: String,
    streaming: bool,
    index: usize,
    expanded: bool,
    theme: &UiTheme,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let colors = theme.colors;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .typography(Typography::Body)
        .text_color(colors.text_muted)
        .child(Disclosure::new(("thinking", index), expanded, *theme).on_toggle(on_toggle).child(if streaming {
            "Thinking…"
        } else {
            "Thought"
        }))
        .when(expanded, |block| {
            block.child(
                div()
                    .debug_selector(move || format!("thinking-text-{index}"))
                    .pl_3()
                    .border_l_2()
                    .border_color(colors.border_subtle)
                    .child(text),
            )
        })
        .into_any_element()
}
