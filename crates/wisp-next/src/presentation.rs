use crate::settings::UiSettings;
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use ratatui::text::Line;

pub struct TranscriptRenderer {
    theme: Theme,
    highlighter: SyntaxHighlighter,
    committed_lines: Vec<Line<'static>>,
    transcript_generation: u64,
}

impl TranscriptRenderer {
    pub fn new(settings: &UiSettings) -> Self {
        Self {
            theme: Theme::load(settings),
            highlighter: SyntaxHighlighter::new(),
            committed_lines: Vec::new(),
            transcript_generation: 0,
        }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn highlighter(&mut self) -> &mut SyntaxHighlighter {
        &mut self.highlighter
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.highlighter.clear();
    }

    pub(crate) fn sync_transcript_generation(&mut self, generation: u64) {
        if self.transcript_generation != generation {
            self.committed_lines.clear();
            self.transcript_generation = generation;
        }
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
