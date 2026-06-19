use super::syntect_bridge::{find_syntax_for_hint, syntect_to_wisp_style};
use crate::line::Line;
use crate::span::Span;
use crate::style::Style;
use crate::theme::Theme;
use std::collections::HashMap;
use std::sync::Mutex;
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxSet;

/// Unified syntax-highlighting facade.
///
/// Results are cached by `(lang, code)` so repeated re-renders
/// (e.g. during streaming) skip the expensive syntect pass.
///
/// The cache uses interior mutability (`Mutex`) so `highlight`
/// works through shared references (`&self`).
pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    cache: Mutex<HashMap<String, HashMap<String, Vec<Line>>>>,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self { syntax_set: two_face::syntax::extra_newlines(), cache: Mutex::new(HashMap::new()) }
    }

    /// Syntax-highlights `code`, caching the result by `(lang, code)`.
    pub fn highlight(&self, code: &str, lang: &str, theme: &Theme) -> Vec<Line> {
        if let Some(cached) = self.cache.lock().unwrap().get(lang).and_then(|m| m.get(code)) {
            return cached.clone();
        }

        let lines = self.render_highlighted_lines(code, lang, theme);
        self.cache.lock().unwrap().entry(lang.to_string()).or_default().insert(code.to_string(), lines.clone());

        lines
    }
}

impl SyntaxHighlighter {
    fn render_highlighted_lines(&self, code: &str, lang: &str, theme: &Theme) -> Vec<Line> {
        let syntax = find_syntax_for_hint(&self.syntax_set, lang);

        let Some(syntax) = syntax else {
            return plain_code_lines(code, theme);
        };

        let syntect_theme = theme.syntect_theme();
        let mut h = HighlightLines::new(syntax, syntect_theme);
        let mut lines = Vec::new();

        for source_line in code.lines() {
            let Ok(ranges) = h.highlight_line(source_line, &self.syntax_set) else {
                lines.push(Line::with_style(source_line, Style::fg(theme.code_fg())));
                continue;
            };

            let mut line = Line::default();
            for (syntect_style, text) in ranges {
                line.push_span(Span::with_style(text, syntect_to_wisp_style(syntect_style)));
            }
            lines.push(line);
        }

        lines
    }
}

fn plain_code_lines(code: &str, theme: &Theme) -> Vec<Line> {
    let style = Style::fg(theme.code_fg());
    code.lines().map(|line| Line::with_style(line, style)).collect()
}
