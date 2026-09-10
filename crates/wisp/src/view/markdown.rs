use crate::theme::Theme;
use crate::view::syntax::SyntaxHighlighter;
use clankerdiff_markdown::MarkdownDocument;
use clankerdiff_ratatui::{MarkdownLayoutOptions, MarkdownRenderer, MarkdownPresentation};
use ratatui::text::Line;

pub fn render_markdown(source: &str, width: u16, theme: &Theme, highlighter: &mut SyntaxHighlighter) -> Vec<Line<'static>> {
    let document = MarkdownDocument::parse(source);
    MarkdownRenderer::new().render_layout(&document, layout_options(width), theme.review(), &mut highlighter.inner)
        .rows().iter().map(|row| row.line.clone()).collect()
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
