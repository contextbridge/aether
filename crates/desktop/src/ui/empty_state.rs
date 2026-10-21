use super::typography::{Typography, TypographyExt};
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{App, Hsla, IntoElement, RenderOnce, SharedString, Window, div, prelude::*};

/// A title and a line of detail, centred in the space they fill.
#[derive(IntoElement)]
pub struct EmptyState {
    title: SharedString,
    detail: SharedString,
    /// The title's color, the text color unless the state is a warning or a failure.
    tone: Option<Hsla>,
    theme: UiTheme,
}

impl EmptyState {
    pub fn new(title: impl Into<SharedString>, detail: impl Into<SharedString>, theme: UiTheme) -> Self {
        Self { title: title.into(), detail: detail.into(), tone: None, theme }
    }

    /// Colors the title `tone`, such as the theme's warning or destructive color.
    pub fn tone(mut self, tone: Hsla) -> Self {
        self.tone = Some(tone);
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .size_full()
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .text_center()
            .child(
                div()
                    .typography(Typography::Title)
                    .text_color(self.tone.unwrap_or(self.theme.colors.text))
                    .child(self.title),
            )
            .child(div().typography(Typography::Body).text_color(self.theme.colors.text_muted).child(self.detail))
    }
}
