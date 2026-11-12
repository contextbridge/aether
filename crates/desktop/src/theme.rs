use crate::ui::ActiveTheme;
use clankerdiff_ui_core::{ReviewTheme, ThemePreference};

pub(crate) fn apply(preference: &ThemePreference) {
    ActiveTheme::current().apply(preference);
}

pub(crate) fn preview(theme: ReviewTheme) {
    ActiveTheme::current().set(theme);
}
