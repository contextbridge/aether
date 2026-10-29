use crate::ui::{UiTheme, window_appearance};
use clankerdiff_ui_core::{Appearance, ReviewTheme, ThemePreference};
use gpui::{App, Global, Window};

#[derive(Debug, Clone)]
pub struct DesktopTheme {
    review: ReviewTheme,
    ui: UiTheme,
}

impl DesktopTheme {
    pub fn new(review: ReviewTheme) -> Self {
        Self { ui: UiTheme::new(&review), review }
    }

    pub fn resolve(preference: &ThemePreference, system: Appearance) -> Self {
        let review = ReviewTheme::builtin(preference.pair.id(preference.appearance(system))).unwrap_or_else(|error| {
            tracing::warn!(%error, "falling back to the default theme");
            ReviewTheme::default()
        });
        Self::new(review)
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn review(&self) -> &ReviewTheme {
        &self.review
    }

    pub fn ui(&self) -> UiTheme {
        self.ui
    }

    fn install(self, window: &mut Window, cx: &mut App) {
        crate::chat::install_markdown(&self.review, &self.ui, cx);
        cx.set_global(self);
        window.refresh();
    }
}

impl Global for DesktopTheme {}

pub(crate) fn apply(preference: &ThemePreference, window: &mut Window, cx: &mut App) {
    DesktopTheme::resolve(preference, window_appearance(window)).install(window, cx);
}

pub(crate) fn preview(theme: ReviewTheme, window: &mut Window, cx: &mut App) {
    DesktopTheme::new(theme).install(window, cx);
}
