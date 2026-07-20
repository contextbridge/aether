use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, HighlightState};
use syntect::parsing::{ParseState, SyntaxReference, SyntaxSet};

const MAX_CACHE_ENTRIES: usize = 512;

/// Work the highlighter did since the last [`SyntaxHighlighter::take_stats`].
/// Byte counters measure input re-processed rather than output produced: a
/// caller that re-highlights a growing block every frame shows up as the whole
/// block's size again and again, whatever the output looks like.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HighlightStats {
    pub calls: u64,
    pub cache_misses: u64,
    pub bytes_highlighted: u64,
}

/// Hash of the `(language, code)` pair a cache entry was built from. Hashing
/// rather than owning the strings means a lookup costs nothing: the hot path
/// runs once per source line per rendered patch.
type CacheKey = (u64, u64);

/// Highlighted lines, shared rather than copied out of the cache: a hit on a
/// long code block would otherwise clone every line of it.
pub type HighlightedLines = Rc<[Line<'static>]>;

/// Where highlighting of a code block got to — enough to continue it later with
/// byte-identical output, so a streaming block can highlight only its new lines
/// instead of the whole block again.
#[derive(Debug, Clone)]
pub struct CodeBlockState {
    highlight: HighlightState,
    parse: ParseState,
}

#[derive(Clone)]
struct CacheEntry {
    lines: HighlightedLines,
    state: Option<CodeBlockState>,
}

pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    cache: HashMap<CacheKey, CacheEntry>,
    /// Insertion order, for evicting the oldest entry when the cache is full.
    /// Entries are not promoted on a hit — callers that re-render the same lines
    /// every frame cache the finished result themselves.
    insertion_order: VecDeque<CacheKey>,
    stats: HighlightStats,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self {
            syntax_set: two_face::syntax::extra_newlines(),
            cache: HashMap::new(),
            insertion_order: VecDeque::new(),
            stats: HighlightStats::default(),
        }
    }

    pub fn take_stats(&mut self) -> HighlightStats {
        std::mem::take(&mut self.stats)
    }

    pub fn highlight(&mut self, code: &str, language: &str, theme: &Theme) -> HighlightedLines {
        self.highlight_seeded(code, language, theme, None).0
    }

    /// Highlights `code`, optionally continuing from [`CodeBlockState`] a
    /// previous call returned, and reports the state it ended in.
    ///
    /// A continuation is transient — its content changes again with the next
    /// chunk — so it bypasses the cache rather than churning it. A fresh block
    /// is cacheable like any finished render, with its end state stored so a
    /// later identical block can continue from the hit.
    pub fn highlight_seeded(
        &mut self,
        code: &str,
        language: &str,
        theme: &Theme,
        seed: Option<CodeBlockState>,
    ) -> (HighlightedLines, Option<CodeBlockState>) {
        self.stats.calls += 1;
        if let Some(seed) = seed {
            return self.highlight_uncached(code, language, theme, Some(seed));
        }
        let key = {
            let mut code_hasher = DefaultHasher::new();
            code.hash(&mut code_hasher);
            let mut language_hasher = DefaultHasher::new();
            language.hash(&mut language_hasher);
            (code_hasher.finish(), language_hasher.finish())
        };
        if let Some(entry) = self.cache.get(&key) {
            return (Rc::clone(&entry.lines), entry.state.clone());
        }
        let (lines, state) = self.highlight_uncached(code, language, theme, None);
        if self.cache.len() >= MAX_CACHE_ENTRIES
            && let Some(oldest) = self.insertion_order.pop_front()
        {
            self.cache.remove(&oldest);
        }
        self.insertion_order.push_back(key);
        self.cache.insert(key, CacheEntry { lines: Rc::clone(&lines), state: state.clone() });
        (lines, state)
    }

    fn highlight_uncached(
        &mut self,
        code: &str,
        language: &str,
        theme: &Theme,
        seed: Option<CodeBlockState>,
    ) -> (HighlightedLines, Option<CodeBlockState>) {
        self.stats.cache_misses += 1;
        self.stats.bytes_highlighted += code.len() as u64;
        let Some(syntax) = find_syntax(&self.syntax_set, language) else {
            let lines: HighlightedLines = Rc::from(
                code.split('\n')
                    .map(|line| Line::styled(line.to_string(), Style::new().fg(theme.code_fg).bg(theme.code_bg)))
                    .collect::<Vec<_>>(),
            );
            return (lines, None);
        };
        let mut highlighter = match seed {
            Some(state) => HighlightLines::from_state(theme.syntect(), state.highlight, state.parse),
            None => HighlightLines::new(syntax, theme.syntect()),
        };
        let lines = highlight_lines(&mut highlighter, code, &self.syntax_set, theme);
        let (highlight, parse) = highlighter.state();
        (Rc::from(lines), Some(CodeBlockState { highlight, parse }))
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.insertion_order.clear();
    }
}

fn highlight_lines(
    highlighter: &mut HighlightLines<'_>,
    code: &str,
    syntax_set: &SyntaxSet,
    theme: &Theme,
) -> Vec<Line<'static>> {
    code.split('\n')
        .map(|source_line| {
            let line_with_ending = format!("{source_line}\n");
            match highlighter.highlight_line(&line_with_ending, syntax_set) {
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

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
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
