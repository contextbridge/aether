use clankerdiff_gpui::style::highlight_style;
use clankerdiff_syntax::{Fingerprint, LanguageHint, SyntaxHighlighter};
use clankerdiff_theme::SyntaxTheme;
use gpui::{App, Global, HighlightStyle};
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

/// The syntax highlighter every transcript shares, so that grammars load once for the app rather
/// than once per thread. Markdown code blocks highlight through it from gpui-base's text view,
/// which needs it to be `Send + Sync`.
#[derive(Clone, Default)]
pub(crate) struct SharedHighlighter(Arc<Mutex<SyntaxHighlighter>>);

impl SharedHighlighter {
    pub(crate) fn global(cx: &mut App) -> Self {
        cx.default_global::<Self>().clone()
    }

    /// Lends the highlighter to `highlight`. Rendering never re-enters it, so it is never lent twice.
    pub(crate) fn with<R>(&self, highlight: impl FnOnce(&mut SyntaxHighlighter) -> R) -> R {
        highlight(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Global for SharedHighlighter {}

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
        highlights.extend(document.line(index).unwrap_or_default().iter().map(|span| {
            (span.range.start + offset..span.range.end + offset, highlight_style(span.foreground, span.font_style))
        }));
        offset += line.len() + 1;
    }
    highlights
}
