use crate::ui::UiTheme;
use clankerdiff_theme::ReviewTheme;
use gpui::{App, Global, SharedString, WindowAppearance};

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

    pub(crate) fn install(pair: ThemePair, appearance: WindowAppearance, cx: &mut App) {
        let theme = Self::new(pair, appearance);
        crate::chat::install_markdown(&theme.review, &theme.ui, cx);
        cx.set_global(theme);
    }
}

impl Global for DesktopTheme {}

/// A light and a dark built-in theme that the app switches between as the
/// system appearance changes.
///
/// Values are clankerdiff built-in theme ids, as listed by [`ReviewTheme::catalog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePair {
    pub light: SharedString,
    pub dark: SharedString,
}

impl ThemePair {
    pub const DEFAULT: Self =
        Self { light: SharedString::new_static("github-light"), dark: SharedString::new_static("github-dark") };

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
