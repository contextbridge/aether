use clankerdiff_gpui::style::highlight_style;
use clankerdiff_syntax::DocumentHighlights;
use clankerdiff_ui_core::SyntaxTheme;
use gpui::HighlightStyle;
use std::ops::Range;

pub(crate) fn highlights(
    source: &str,
    document: &DocumentHighlights,
    theme: &SyntaxTheme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut offset = 0;
    let mut highlights = Vec::new();
    for (index, line) in source.split('\n').enumerate() {
        highlights.extend(document.line(index).unwrap_or_default().iter().filter_map(|span| {
            let style = theme.highlight(span.highlight)?;
            Some((
                span.range.start + offset..span.range.end + offset,
                highlight_style(style.foreground, style.font_style),
            ))
        }));
        offset += line.len() + 1;
    }
    highlights
}
