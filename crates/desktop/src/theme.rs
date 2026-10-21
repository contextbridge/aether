use clankerdiff_gpui::ui::prelude::UiTheme;
use clankerdiff_theme::ReviewTheme;
use gpui::{App, Global, SharedString, WindowAppearance};
use gpui_base::input::InputEditorStyle;
use serde::{Deserialize, Serialize};

/// The app-wide theme, installed as a gpui [`Global`].
///
/// It holds the [`ReviewTheme`] (handed to clankerdiff views such as the diff
/// viewer) and the [`UiTheme`] derived from it, which every other view paints with.
#[derive(Debug, Clone)]
pub struct DesktopTheme {
    pair: ThemePair,
    review: ReviewTheme,
    ui: UiTheme,
}

impl DesktopTheme {
    /// Resolves the theme in `pair` that matches `appearance`.
    ///
    /// An id that is not a clankerdiff built-in falls back to clankerdiff's default theme.
    pub fn new(pair: ThemePair, appearance: WindowAppearance) -> Self {
        let id = pair.theme_id(appearance).to_owned();
        let review = ReviewTheme::builtin(&id).unwrap_or_else(|error| {
            tracing::warn!(%error, "falling back to the default theme");
            ReviewTheme::default()
        });
        Self { pair, ui: UiTheme::new(&review), review }
    }

    /// Returns the installed theme.
    ///
    /// # Panics
    ///
    /// Panics if no window has installed a theme yet.
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn pair(&self) -> &ThemePair {
        &self.pair
    }

    pub fn review(&self) -> &ReviewTheme {
        &self.review
    }

    pub fn ui(&self) -> UiTheme {
        self.ui
    }

    /// How text inputs paint in this theme.
    pub(crate) fn editor_style(&self) -> InputEditorStyle {
        let colors = self.ui.colors;
        let mut selection = colors.accent;
        selection.a = 0.3;
        InputEditorStyle {
            foreground: colors.text,
            muted_foreground: colors.text_muted,
            background: colors.surface,
            border: colors.border,
            selection,
            caret: colors.text,
            ..InputEditorStyle::default()
        }
    }

    /// Installs the theme in `pair` that matches `appearance`.
    pub(crate) fn install(pair: ThemePair, appearance: WindowAppearance, cx: &mut App) {
        cx.set_global(Self::new(pair, appearance));
    }
}

impl Global for DesktopTheme {}

/// A light and a dark built-in theme that the app switches between as the
/// system appearance changes.
///
/// Values are clankerdiff built-in theme ids, as listed by [`ReviewTheme::catalog`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemePair {
    pub light: SharedString,
    pub dark: SharedString,
}

impl ThemePair {
    pub const DEFAULT: Self =
        Self { light: SharedString::new_static("github-light"), dark: SharedString::new_static("github-dark") };

    /// Returns the id of the theme in this pair that matches `appearance`.
    pub fn theme_id(&self, appearance: WindowAppearance) -> &str {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => &self.light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => &self.dark,
        }
    }
}

impl Default for ThemePair {
    fn default() -> Self {
        Self::DEFAULT
    }
}
