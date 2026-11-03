use crate::theme::Theme;
use clankerdiff_ratatui::highlighted_line;
use clankerdiff_syntax::{LanguageHint, analyze};
use ratatui::style::Style;
use ratatui::text::Line;
use std::rc::Rc;

pub type HighlightedLines = Rc<[Line<'static>]>;

pub fn highlight(code: &str, language: &str, theme: &Theme) -> HighlightedLines {
    let syntax = analyze(LanguageHint::InfoString(language), code);
    let base = Style::from((theme.code_fg, theme.code_bg));
    code.split('\n')
        .enumerate()
        .map(|(index, source)| {
            highlighted_line(source, syntax.highlights.line(index).unwrap_or_default(), &theme.review().syntax, base)
        })
        .collect::<Vec<_>>()
        .into()
}
