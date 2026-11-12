use clankerdiff_rouge::style;
use clankerdiff_rouge_shell::rouge::core::SpanStyle;
use clankerdiff_syntax::DocumentHighlights;
use clankerdiff_ui_core::SyntaxTheme;
use std::ops::Range;

pub(crate) fn highlights(
    source: &str,
    document: &DocumentHighlights,
    theme: &SyntaxTheme,
) -> Vec<(Range<usize>, SpanStyle)> {
    let mut offset = 0;
    let mut highlights = Vec::new();
    for (index, line) in source.split('\n').enumerate() {
        let spans = style::highlights(line, document.line(index).unwrap_or_default(), theme);
        highlights.extend(spans.into_iter().map(|(range, style)| (range.start + offset..range.end + offset, style)));
        offset += line.len() + 1;
    }
    highlights
}
