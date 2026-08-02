use crate::components::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use syntect::parsing::{SyntaxReference, SyntaxSet};

const MAX_CACHE_ENTRIES: usize = 512;

/// Hash of the `(language, code)` pair a cache entry was built from. Hashing
/// rather than owning the strings means a lookup costs nothing: the hot path
/// runs once per source line per rendered patch.
type CacheKey = (u64, u64);

/// Highlighted lines, shared rather than copied out of the cache: a hit on a
/// long code block would otherwise clone every line of it.
pub type HighlightedLines = Rc<[Line<'static>]>;

pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    cache: HashMap<CacheKey, HighlightedLines>,
    /// Insertion order, for evicting the oldest entry when the cache is full.
    /// Entries are not promoted on a hit — callers that re-render the same lines
    /// every frame cache the finished result themselves.
    insertion_order: VecDeque<CacheKey>,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self { syntax_set: two_face::syntax::extra_newlines(), cache: HashMap::new(), insertion_order: VecDeque::new() }
    }

    pub fn highlight(&mut self, code: &str, language: &str, theme: &Theme) -> HighlightedLines {
        let key = cache_key(code, language);
        if let Some(lines) = self.cache.get(&key) {
            return Rc::clone(lines);
        }
        let lines: HighlightedLines = Rc::from(self.render(code, language, theme));
        if self.cache.len() >= MAX_CACHE_ENTRIES
            && let Some(oldest) = self.insertion_order.pop_front()
        {
            self.cache.remove(&oldest);
        }
        self.insertion_order.push_back(key);
        self.cache.insert(key, Rc::clone(&lines));
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
            .map(|source_line| {
                let line_with_ending = format!("{source_line}\n");
                match highlighter.highlight_line(&line_with_ending, &self.syntax_set) {
                    Ok(ranges) => Line::from(
                        ranges
                            .into_iter()
                            .filter_map(|(style, text)| {
                                let text = text.strip_suffix('\n').unwrap_or(text);
                                (!text.is_empty()).then(|| Span::styled(text.to_string(), style_from_syntect(style)))
                            })
                            .collect::<Vec<_>>(),
                    ),
                    Err(_) => Line::styled(source_line.to_string(), Style::new().fg(theme.code_fg)),
                }
            })
            .collect()
    }
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

fn cache_key(code: &str, language: &str) -> CacheKey {
    (hash_of(code), hash_of(language))
}

fn hash_of(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn find_syntax<'a>(syntax_set: &'a SyntaxSet, hint: &str) -> Option<&'a SyntaxReference> {
    let language = hint
        .split(|character: char| character.is_whitespace() || character == ',')
        .find(|part| !part.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let normalized = match language.as_str() {
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
        language => language,
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
