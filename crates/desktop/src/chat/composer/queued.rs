use crate::ui::tokens::ICON_SIZE_SMALL;
use crate::ui::{Icon, IconName, Typography, TypographyExt, UiTheme};
use client_core::session_core::QueuedPrompt;
use gpui::{Div, div, prelude::*};

pub(crate) fn queued_prompts(prompts: &[QueuedPrompt], theme: &UiTheme) -> Div {
    let shown = if prompts.len() > MAX_QUEUED_ROWS + 1 { MAX_QUEUED_ROWS } else { prompts.len() };
    let mut list =
        div().debug_selector(|| "queued-prompts".to_owned()).flex().flex_col().gap_1().typography(Typography::Small);
    for prompt in &prompts[..shown] {
        let text = prompt.submission.text.lines().next().unwrap_or_default().to_owned();
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(Icon::new(IconName::ChevronRight, theme.colors.text_muted).size(ICON_SIZE_SMALL))
                .child(div().flex_none().text_color(theme.colors.text_muted).child("Queued"))
                .child(div().flex_1().min_w_0().truncate().text_color(theme.colors.text_secondary).child(text)),
        );
    }
    if shown < prompts.len() {
        list = list
            .child(div().text_color(theme.colors.text_muted).child(format!("{} more queued", prompts.len() - shown)));
    }
    list
}

const MAX_QUEUED_ROWS: usize = 3;
