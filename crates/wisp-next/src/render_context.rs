use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

/// Shared rendering services, borrowed for the duration of one draw.
///
/// `theme_generation` lets a surface cache styled text across frames and
/// rebuild it only when the theme actually changes.
pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub highlighter: &'a mut SyntaxHighlighter,
    pub theme_generation: u64,
}
