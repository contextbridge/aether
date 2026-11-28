use crate::theme::Theme;
use clankerdiff_ratatui::MarkdownDocument;
use clankerdiff_ratatui::{MarkdownLayoutOptions, MarkdownRenderer};
use ratatui::text::Line;

pub(crate) fn render_markdown(source: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let document = MarkdownDocument::parse(source);
    MarkdownRenderer::new().render_lines(&document, layout_options(width), theme.review()).to_vec()
}

pub(crate) fn layout_options(width: u16) -> MarkdownLayoutOptions {
    MarkdownLayoutOptions { width, ..MarkdownLayoutOptions::default() }
}
