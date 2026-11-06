use crate::ui::ActiveTheme;
use clankerdiff_ui_core::{ReviewTheme, ThemePreference};
use gpui::{App, Window};

pub(crate) fn apply(preference: &ThemePreference, window: &mut Window, cx: &mut App) {
    ActiveTheme::apply(preference, window, cx);
    install_markdown(cx);
}

pub(crate) fn preview(theme: ReviewTheme, window: &mut Window, cx: &mut App) {
    ActiveTheme::install(theme, window, cx);
    install_markdown(cx);
}

fn install_markdown(cx: &mut App) {
    let theme = ActiveTheme::global(cx);
    let (review, ui) = (theme.review().clone(), theme.ui());
    crate::chat::install_markdown(&review, &ui, cx);
}
