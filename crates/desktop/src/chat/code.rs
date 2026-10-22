use clankerdiff_gpui::style::highlight_style;
use clankerdiff_syntax::{Fingerprint, HighlightSpan, LanguageHint, SyntaxHighlighter};
use clankerdiff_theme::SyntaxTheme;
#[cfg(not(feature = "testing"))]
use gpui::StyledText;
use gpui::{AnyElement, HighlightStyle, IntoElement, SharedString};
use std::ops::Range;

/// Styled source text, observable at paint time through the desktop integration harness.
pub(crate) fn styled_text(
    text: impl Into<SharedString>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> AnyElement {
    #[cfg(feature = "testing")]
    {
        crate::testing::observe_text(text.into(), highlights).into_any_element()
    }
    #[cfg(not(feature = "testing"))]
    {
        StyledText::new(text).with_highlights(highlights).into_any_element()
    }
}

/// Highlights a complete source document, retaining context across lines.
pub(crate) fn highlight(
    source: &str,
    hint: LanguageHint<'_>,
    theme: &SyntaxTheme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Ok(document) =
        highlighter.with_theme(theme).highlight_document(Fingerprint::of([source.as_bytes()]), hint, || source)
    else {
        return Vec::new();
    };
    let mut offset = 0;
    let mut highlights = Vec::new();
    for (index, line) in source.split('\n').enumerate() {
        highlights.extend(span_styles(document.line(index).unwrap_or_default(), offset));
        offset += line.len() + 1;
    }
    highlights
}

/// Converts line-relative syntax spans to GPUI byte ranges in the displayed text.
pub(crate) fn span_styles(spans: &[HighlightSpan], offset: usize) -> Vec<(Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .map(|span| {
            (span.range.start + offset..span.range.end + offset, highlight_style(span.foreground, span.font_style))
        })
        .collect()
}
