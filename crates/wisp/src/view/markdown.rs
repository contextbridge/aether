use crate::theme::Theme;
use clankerdiff_ratatui::MarkdownDocument;
use clankerdiff_ratatui::{MarkdownLayoutOptions, MarkdownPresentation, MarkdownRenderer};
use ratatui::text::Line;

pub(crate) fn render_markdown(source: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let document = MarkdownDocument::parse(source);
    MarkdownRenderer::new()
        .render_layout(&document, layout_options(width), theme.review())
        .rows()
        .iter()
        .map(|row| row.line.clone())
        .collect()
}

pub(crate) fn layout_options(width: u16) -> MarkdownLayoutOptions {
    MarkdownLayoutOptions {
        width,
        block_spacing: true,
        presentation: MarkdownPresentation::Rendered,
        wrap: true,
        heading_markers: true,
        preserve_source_gaps: false,
        tab_width: 4,
    }
}
