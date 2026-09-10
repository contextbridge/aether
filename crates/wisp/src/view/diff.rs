use crate::git_review::FileDiff;
use crate::theme::Theme;
use crate::view::syntax::SyntaxHighlighter;
use clankerdiff_ratatui::{DiffPreviewOptions, DiffPreviewState};
use ratatui::text::Line;

/// Renders a sealed tool diff; the transcript cache retains the resulting rows.
pub fn render_diff(
    file: &FileDiff,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    DiffPreviewState::new(file.clone())
        .render(width, theme.review(), &mut highlighter.inner, DiffPreviewOptions::default())
        .to_vec()
}
