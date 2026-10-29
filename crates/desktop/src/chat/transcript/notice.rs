use crate::ui::{Icon, IconName, Typography, TypographyExt, UiTheme};
use gpui::{AnyElement, Div, Hsla, div, prelude::*};

pub(crate) fn notice(text: &str, theme: &UiTheme) -> AnyElement {
    line(IconName::Info, theme.colors.text_muted, text).into_any_element()
}

pub(crate) fn error(index: usize, text: &str, theme: &UiTheme) -> AnyElement {
    line(IconName::TriangleAlert, theme.colors.destructive, text)
        .debug_selector(move || format!("transcript-error-{index}"))
        .into_any_element()
}

pub(crate) fn line(icon: IconName, color: Hsla, text: &str) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .typography(Typography::Small)
        .text_color(color)
        .child(Icon::new(icon, color))
        .child(div().flex_1().min_w_0().child(text.to_owned()))
}
