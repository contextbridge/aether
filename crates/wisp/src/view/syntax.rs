use crate::theme::Theme;
use clankerdiff_ratatui::highlighted_line;
use clankerdiff_syntax::LanguageHint;
use clankerdiff_theme::Fingerprint;
use ratatui::style::Style;
use ratatui::text::Line;
use std::rc::Rc;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HighlightStats {
    pub calls: u64,
    pub cache_misses: u64,
    pub bytes_highlighted: u64,
}

pub type HighlightedLines = Rc<[Line<'static>]>;

#[derive(Default)]
pub struct SyntaxHighlighter {
    pub(crate) inner: clankerdiff_syntax::SyntaxHighlighter,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_stats(&mut self) -> HighlightStats {
        let stats = self.inner.take_stats();
        HighlightStats { calls: stats.calls, cache_misses: stats.misses, bytes_highlighted: stats.bytes as u64 }
    }

    pub fn highlight(&mut self, code: &str, language: &str, theme: &Theme) -> HighlightedLines {
        let highlights = self.inner.with_theme(&theme.review().syntax).highlight_document(
            Fingerprint::of([code.as_bytes()]),
            LanguageHint::InfoString(language),
            || code,
        );
        let base = Style::new().fg(theme.code_fg).bg(theme.code_bg);
        match highlights {
            Ok(highlights) => code
                .split('\n')
                .enumerate()
                .map(|(index, source)| highlighted_line(source, highlights.line(index).unwrap_or_default(), base))
                .collect::<Vec<_>>()
                .into(),
            Err(error) => {
                tracing::warn!(%error, "Could not highlight source snippet");
                vec![Line::styled(format!("Could not highlight source: {error}"), base)].into()
            }
        }
    }
}
