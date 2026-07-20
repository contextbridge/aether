use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::{HashMap, VecDeque};
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use syntect::parsing::{SyntaxReference, SyntaxSet};

pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    cache: HashMap<(String, String), Vec<Line<'static>>>,
    insertion_order: VecDeque<(String, String)>,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self { syntax_set: two_face::syntax::extra_newlines(), cache: HashMap::new(), insertion_order: VecDeque::new() }
    }

    pub fn highlight(&mut self, code: &str, language: &str, theme: &Theme) -> Vec<Line<'static>> {
        let key = (language.to_string(), code.to_string());
        if let Some(lines) = self.cache.get(&key) {
            return lines.clone();
        }
        let lines = self.render(code, language, theme);
        if self.cache.len() == MAX_CACHE_ENTRIES
            && let Some(oldest) = self.insertion_order.pop_front()
        {
            self.cache.remove(&oldest);
        }
        self.insertion_order.push_back(key.clone());
        self.cache.insert(key, lines.clone());
        lines
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.insertion_order.clear();
    }

    fn render(&self, code: &str, language: &str, theme: &Theme) -> Vec<Line<'static>> {
        let Some(syntax) = find_syntax(&self.syntax_set, language) else {
            return plain_lines(code, theme);
        };
        let mut highlighter = HighlightLines::new(syntax, theme.syntect());
        code.split('\n')
            .map(|source_line| match highlighter.highlight_line(source_line, &self.syntax_set) {
                Ok(ranges) => Line::from(
                    ranges
                        .into_iter()
                        .map(|(style, text)| Span::styled(text.to_string(), style_from_syntect(style)))
                        .collect::<Vec<_>>(),
                ),
                Err(_) => Line::styled(source_line.to_string(), Style::new().fg(theme.code_fg)),
            })
            .collect()
    }
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

const MAX_CACHE_ENTRIES: usize = 128;

fn find_syntax<'a>(syntax_set: &'a SyntaxSet, hint: &str) -> Option<&'a SyntaxReference> {
    let normalized = match hint.to_ascii_lowercase().as_str() {
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
    };
    if normalized.is_empty() {
        return None;
    }
    syntax_set.find_syntax_by_extension(normalized).or_else(|| syntax_set.find_syntax_by_token(normalized))
}

fn plain_lines(code: &str, theme: &Theme) -> Vec<Line<'static>> {
    code.split('\n')
        .map(|line| Line::styled(line.to_string(), Style::new().fg(theme.code_fg).bg(theme.code_bg)))
        .collect()
}

fn style_from_syntect(style: syntect::highlighting::Style) -> Style {
    let mut modifiers = Modifier::empty();
    if style.font_style.contains(FontStyle::BOLD) {
        modifiers.insert(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        modifiers.insert(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        modifiers.insert(Modifier::UNDERLINED);
    }
    Style::new().fg(Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)).add_modifier(modifiers)
}
