use crate::ui::ActiveReviewTheme;
use clankerdiff_ui_core::{ReviewTheme, ThemePreference};

pub(crate) fn apply(preference: &ThemePreference) {
    ActiveReviewTheme::current().apply(preference);
}

pub(crate) fn preview(theme: ReviewTheme) {
    ActiveReviewTheme::current().set(theme);
}
