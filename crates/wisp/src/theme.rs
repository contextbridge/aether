use crate::settings::{ThemeSettings, load_theme_file};
use clankerdiff_ratatui::{RatatuiTheme, RatatuiUiTheme, composite_color, layered_style};
use clankerdiff_ratatui::theme::{ReviewTheme, SelectionState, ThemeId};
use ratatui::style::{Color, Style};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Theme {
    pub text_primary: Color,
    pub text_secondary: Color,
    pub background: Color,
    pub surface: Color,
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
    Theme(#[from] clankerdiff_ratatui::theme::ThemeError),
    #[error("Invalid theme file name: {0}")]
    InvalidFile(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeApplicationError {
    #[error(transparent)]
    Load(#[from] ThemeLoadError),
    #[error("Could not save theme settings: {0}")]
    Save(#[source] std::io::Error),
    #[error("Theme task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

impl Theme {
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

    pub fn surface_style(&self) -> Style {
        let ui = self.review.ui;
        layered_style(ui.text, ui.surface, ui.canvas)
    }

    pub fn selection_style(&self, state: SelectionState) -> Style {
        RatatuiUiTheme::from(&self.review.ui).selection_style(state)
    }

    pub fn from_review(review: ReviewTheme) -> Self {
        let native = RatatuiTheme::from(&review);
        let ui = native.ui;
        let color = |value| composite_color(value, review.diff.background);
        Self {
            text_primary: ui.text,
            text_secondary: ui.text_secondary,
            background: ui.canvas,
            surface: ui.surface,
            accent: ui.accent,
            heading: color(review.markdown.heading),
            link: color(review.markdown.link),
            blockquote: color(review.markdown.quote),
            code_fg: color(review.markdown.code),
            code_bg: color(review.markdown.code_background),
            success: ui.positive,
            warning: ui.warning,
            error: ui.destructive,
            info: ui.info,
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
