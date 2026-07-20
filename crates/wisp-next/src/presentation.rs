use crate::settings::UiSettings;
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

pub struct TranscriptRenderer {
    theme: Theme,
    highlighter: SyntaxHighlighter,
}

impl TranscriptRenderer {
    pub fn new(settings: &UiSettings) -> Self {
        Self { theme: Theme::load(settings), highlighter: SyntaxHighlighter::new() }
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
}
