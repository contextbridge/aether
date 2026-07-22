use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::app::{HistoryItem, HistoryKind};
use crate::markdown::render_markdown;
use crate::render::indent_lines;
use crate::settings::{ResolvedStatusLineSettings, UiSettings, resolve_status_line_settings};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use ratatui::text::Line;

pub struct TranscriptRenderer {
    theme: Theme,
    highlighter: SyntaxHighlighter,
    committed_lines: Vec<Line<'static>>,
    transcript_generation: u64,
    pending_markdown_cache: HashMap<(u64, u16), Vec<Line<'static>>>,
    settings: ResolvedStatusLineSettings,
}

impl TranscriptRenderer {
    pub fn new(settings: &UiSettings) -> Self {
        Self {
            theme: Theme::load(settings),
            highlighter: SyntaxHighlighter::new(),
            committed_lines: Vec::new(),
            transcript_generation: 0,
            pending_markdown_cache: HashMap::new(),
            settings: resolve_status_line_settings(settings),
        }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn settings(&self) -> &ResolvedStatusLineSettings {
        &self.settings
    }

    pub fn highlighter(&mut self) -> &mut SyntaxHighlighter {
        &mut self.highlighter
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.highlighter.clear();
        self.pending_markdown_cache.clear();
    }

    pub(crate) fn sync_transcript_generation(&mut self, generation: u64) {
        if self.transcript_generation != generation {
            self.committed_lines.clear();
            self.pending_markdown_cache.clear();
            self.transcript_generation = generation;
        }
    }

    pub(crate) fn pending_history_lines(
        &mut self,
        items: &[HistoryItem],
        previous_kind: Option<HistoryKind>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut previous = previous_kind;
        for item in items {
            let kind = item.kind();
            if previous.is_some_and(|value| value != kind) {
                lines.push(Line::default());
            }
            lines.extend(self.pending_item_lines(item, width, padding, spinner_tick));
            previous = Some(kind);
        }
        lines
    }

    fn pending_item_lines(
        &mut self,
        item: &HistoryItem,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let HistoryItem::Text(text) = item else {
            return self.item_lines(item, width, padding, spinner_tick);
        };
        let content_width = width.saturating_sub(u16::try_from(padding.saturating_mul(2)).unwrap_or(u16::MAX)).max(1);
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = (hasher.finish(), content_width);
        self.pending_markdown_cache
            .entry(key)
            .or_insert_with(|| {
                indent_lines(render_markdown(text, content_width, &self.theme, &mut self.highlighter), padding)
            })
            .clone()
    }
    pub(crate) fn append_committed_lines(&mut self, lines: Vec<Line<'static>>) {
        self.committed_lines.extend(lines);
    }

    pub(crate) fn committed_lines(&self) -> &[Line<'static>] {
        &self.committed_lines
    }

    pub(crate) fn take_committed_overflow(&mut self, visible_lines: usize) -> Vec<Line<'static>> {
        let overflow = self.committed_lines.len().saturating_sub(visible_lines);
        self.committed_lines.drain(..overflow).collect()
    }
}
