use clankerdiff_gpui::style::highlight_style;
use clankerdiff_syntax::{Fingerprint, LanguageHint, SyntaxHighlighter};
use clankerdiff_ui_core::SyntaxTheme;
use gpui::{App, Global, HighlightStyle};
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Clone, Default)]
pub(crate) struct SharedHighlighter(Arc<Mutex<SyntaxHighlighter>>);

impl SharedHighlighter {
    pub(crate) fn global(cx: &mut App) -> Self {
        cx.default_global::<Self>().clone()
    }

    pub(crate) fn with<T>(&self, highlight: impl FnOnce(&mut SyntaxHighlighter) -> T) -> T {
        highlight(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Global for SharedHighlighter {}

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
