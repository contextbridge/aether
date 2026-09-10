use crate::settings::{ThemeSettings, UiSettings, load_theme_file};
use clankerdiff_ratatui::{RatatuiTheme, composite_color};
use clankerdiff_theme::{NoticeTone, ReviewTheme, ThemeId};
use ratatui::style::Color;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Theme {
    pub text_primary: Color,
    pub text_secondary: Color,
    pub background: Color,
    pub sidebar_bg: Color,
    pub accent: Color,
    pub heading: Color,
    pub link: Color,
    pub blockquote: Color,
    pub code_fg: Color,
    pub code_bg: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub muted: Color,
    pub diff_added_fg: Color,
    pub diff_added_bg: Color,
    pub diff_removed_fg: Color,
    pub diff_removed_bg: Color,
    review: Arc<ReviewTheme>,
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeLoadError {
    #[error("Could not read theme: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Theme(#[from] clankerdiff_theme::ThemeError),
    #[error("Invalid theme file name: {0}")]
    InvalidFile(String),
}

impl Theme {
    pub fn load(settings: &UiSettings) -> Self {
        Self::load_selection(&settings.theme).unwrap_or_else(|error| {
            tracing::warn!(%error, "Could not load selected theme");
            Self::default()
        })
    }

    pub fn load_selection(selection: &ThemeSettings) -> Result<Self, ThemeLoadError> {
        match selection {
            ThemeSettings::Builtin { id } => Ok(Self::from_review(ReviewTheme::builtin(id)?)),
            ThemeSettings::File { file } => load_theme_file(file),
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self, ThemeLoadError> {
        if path.extension().is_none_or(|extension| extension != "json") {
            return Err(ThemeLoadError::InvalidFile(path.display().to_string()));
        }
        let bytes = std::fs::read(path)?;
        Ok(Self::from_review(ReviewTheme::from_bytes(
            ThemeId::Custom(path.file_name().unwrap_or_default().to_string_lossy().into_owned()),
            &bytes,
        )?))
    }

    pub fn review(&self) -> &ReviewTheme {
        &self.review
    }

    pub fn from_review(review: ReviewTheme) -> Self {
        let native = RatatuiTheme::from(&review);
        let ui = native.ui;
        let color = |value| composite_color(value, review.diff.background);
        Self {
            text_primary: ui.text,
            text_secondary: ui.text_muted,
            background: ui.canvas,
            sidebar_bg: ui.surface_selected,
            accent: ui.accent,
            heading: color(review.markdown.heading),
            link: color(review.markdown.link),
            blockquote: color(review.markdown.quote),
            code_fg: color(review.markdown.code),
            code_bg: color(review.markdown.code_background),
            success: ui.positive,
            warning: ui.notice_style(NoticeTone::Warning).fg.unwrap_or(ui.text),
            error: ui.destructive,
            info: ui.notice_style(NoticeTone::Info).fg.unwrap_or(ui.text),
            muted: ui.text_muted,
            diff_added_fg: native.addition,
            diff_added_bg: native.addition_background,
            diff_removed_fg: native.deletion,
            diff_removed_bg: native.deletion_background,
            review: Arc::new(review),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_review(ReviewTheme::default())
    }
}
