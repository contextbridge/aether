use rouge::core::SpanStyle;
use rouge::syntax::{DocumentHighlights, SyntaxTheme};
use rouge::widgets::syntax_spans;
use std::ops::Range;

pub(crate) fn highlights(
    source: &str,
    document: &DocumentHighlights,
    theme: &SyntaxTheme,
) -> Vec<(Range<usize>, SpanStyle)> {
    let mut offset = 0;
    let mut highlights = Vec::new();
    for (index, line) in source.split('\n').enumerate() {
        let spans = syntax_spans(line, document.line(index).unwrap_or_default(), theme);
        highlights.extend(spans.into_iter().map(|(range, style)| (range.start + offset..range.end + offset, style)));
        offset += line.len() + 1;
    }
    highlights
}
