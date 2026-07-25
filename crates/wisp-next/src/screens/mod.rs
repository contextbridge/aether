pub mod git_diff;
pub mod plan_review;

use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

/// Work a screen wants performed off the UI thread. Only the git-diff screen
/// produces effects today, so this is its effect type directly.
pub type ScreenEffect = git_diff::GitDiffEffect;

/// The result of a [`ScreenEffect`], routed back to the screen that asked for it.
pub type ScreenEvent = git_diff::GitDiffEvent;

/// A full-screen view that takes over the whole terminal and owns all input.
///
/// At most one is open at a time; `App` stores it as a trait object and never
/// needs to know which screen it is.
pub trait Screen {
    fn on_key(&mut self, key: KeyEvent) -> ScreenOutcome;

    fn on_event(&mut self, event: ScreenEvent) -> Option<ScreenEffect> {
        let _ = event;
        None
    }

    fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) {
        let _ = (action, row, column);
    }

    /// Draws the screen, returning where the terminal cursor should sit.
    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position>;

    /// Releases any resources the screen holds, such as an unanswered request.
    fn cancel(&mut self) {}
}

pub enum ScreenOutcome {
    None,
    Close,
    Effect(ScreenEffect),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseAction {
    ScrollUp,
    ScrollDown,
    Click,
}

/// Shared rendering services. `theme_generation` lets screens cache styled text
/// across frames and rebuild it only when the theme actually changes.
pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub highlighter: &'a mut SyntaxHighlighter,
    pub theme_generation: u64,
}
