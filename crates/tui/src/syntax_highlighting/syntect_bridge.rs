use crossterm::style::Color;
use syntect::highlighting::FontStyle;
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::style::Style;

pub(crate) fn find_syntax_for_hint<'a>(syntax_set: &'a SyntaxSet, hint: &str) -> Option<&'a SyntaxReference> {
    if hint.is_empty() {
        return None;
    }

    let normalized = normalize_lang_hint(hint);

    syntax_set.find_syntax_by_extension(normalized).or_else(|| syntax_set.find_syntax_by_token(normalized))
}

fn normalize_lang_hint(hint: &str) -> &str {
    let hint_lower = hint.to_lowercase();
    match hint_lower.as_str() {
        "typescript" => "ts",
        "typescriptreact" => "tsx",
        "javascript" | "jsx" => "js",
        "python" => "py",
        "rust" => "rs",
        "c99" | "c11" => "c",
        "c++" | "cxx" | "cc" => "cpp",
        "c#" | "csharp" => "cs",
        "ruby" => "rb",
        "kotlin" | "kts" => "kt",
        "shell" | "bash" | "zsh" => "sh",
        "yml" => "yaml",
        "markdown" => "md",
        _ => hint,
    }
}

pub(crate) fn syntect_to_wisp_style(s: syntect::highlighting::Style) -> Style {
    let fg = Color::Rgb { r: s.foreground.r, g: s.foreground.g, b: s.foreground.b };

    let mut style = Style::fg(fg);
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.bold();
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.italic();
    }
    if s.font_style.contains(FontStyle::UNDERLINE) {
        style = style.underline();
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_grammar_is_loaded() {
        let ss = two_face::syntax::extra_newlines();
        assert!(ss.find_syntax_by_extension("ts").is_some(), "TypeScript grammar should be found by extension 'ts'");
    }

    #[test]
    fn tsx_grammar_is_loaded() {
        let ss = two_face::syntax::extra_newlines();
        assert!(ss.find_syntax_by_extension("tsx").is_some(), "TSX grammar should be found by extension 'tsx'");
    }

    #[test]
    fn find_syntax_resolves_typescript_hints() {
        let ss = two_face::syntax::extra_newlines();
        for hint in &["ts", "typescript", "TypeScript"] {
            let syn = find_syntax_for_hint(&ss, hint);
            assert!(syn.is_some(), "should resolve hint '{hint}'");
            assert_eq!(syn.unwrap().name, "TypeScript");
        }
    }

    #[test]
    fn find_syntax_resolves_tsx_hints() {
        let ss = two_face::syntax::extra_newlines();
        for hint in &["tsx", "typescriptreact", "TypeScriptReact"] {
            let syn = find_syntax_for_hint(&ss, hint);
            assert!(syn.is_some(), "should resolve hint '{hint}'");
            assert_eq!(syn.unwrap().name, "TypeScriptReact");
        }
    }

    #[test]
    fn javascript_still_resolves() {
        let ss = two_face::syntax::extra_newlines();
        for hint in &["js", "javascript", "jsx"] {
            assert!(find_syntax_for_hint(&ss, hint).is_some(), "should resolve hint '{hint}'");
        }
    }

    #[test]
    fn empty_hint_returns_none() {
        let ss = two_face::syntax::extra_newlines();
        assert!(find_syntax_for_hint(&ss, "").is_none());
    }
}
