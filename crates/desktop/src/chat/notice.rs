use crate::ui::{Icon, IconName, Typography, TypographyExt};
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{AnyElement, div, prelude::*};

/// Something the app, not the agent, says in the transcript, such as a compaction, a cleared
/// context or an error.
pub(crate) fn notice(text: &str, theme: &UiTheme) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_2()
        .typography(Typography::Small)
        .text_color(theme.colors.text_muted)
        .child(Icon::new(IconName::Info, theme.colors.text_muted))
        .child(div().flex_1().min_w_0().child(text.to_owned()))
        .into_any_element()
}
