use std::sync::Arc;

use crate::theme::Theme;

#[cfg(feature = "syntax")]
use crate::syntax_highlighting::SyntaxHighlighter;

#[doc = include_str!("../docs/view_context.md")]
#[derive(Clone)]
pub struct ViewContext {
    /// The size this context allows the component to draw into. This is *not*
    /// the terminal size — parents pass child contexts whose size is the slice
    /// of the terminal allocated to the child.
    pub size: Size,
    pub theme: Arc<Theme>,
    #[cfg(feature = "syntax")]
    pub(crate) highlighter: Arc<SyntaxHighlighter>,
}

/// The size, in columns and rows, a component is permitted to draw into.
///
/// Parents produce child sizes by slicing their own (via
/// [`ViewContext::with_width`], [`ViewContext::with_height`], or
/// [`ViewContext::inset`]). Children should treat the size as authoritative
/// and never assume the full terminal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

/// Edge insets used to shrink a [`ViewContext`] to a smaller size.
///
/// Used by parents that want to render a child inside a padded box: subtract
/// the insets from the parent size to get the child's allocated size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Insets {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

impl Insets {
    pub fn new(left: u16, right: u16, top: u16, bottom: u16) -> Self {
        Self { left, right, top, bottom }
    }

    /// Insets that are equal on all four sides.
    pub fn all(amount: u16) -> Self {
        Self { left: amount, right: amount, top: amount, bottom: amount }
    }

    /// Insets that are equal on opposing sides.
    pub fn symmetric(horizontal: u16, vertical: u16) -> Self {
        Self { left: horizontal, right: horizontal, top: vertical, bottom: vertical }
    }

    /// Insets that only affect the horizontal axis.
    pub fn horizontal(amount: u16) -> Self {
        Self::symmetric(amount, 0)
    }

    /// Insets that only affect the vertical axis.
    pub fn vertical(amount: u16) -> Self {
        Self::symmetric(0, amount)
    }
}

impl ViewContext {
    pub fn new(size: impl Into<Size>) -> Self {
        Self::new_with_theme(size, Theme::default())
    }

    pub fn new_with_theme(size: impl Into<Size>, theme: Theme) -> Self {
        Self {
            size: size.into(),
            theme: Arc::new(theme),
            #[cfg(feature = "syntax")]
            highlighter: Arc::new(SyntaxHighlighter::new()),
        }
    }

    #[cfg(feature = "syntax")]
    pub fn highlighter(&self) -> &SyntaxHighlighter {
        &self.highlighter
    }

    /// Clone this context with a new size, preserving theme state.
    pub fn with_size(&self, size: impl Into<Size>) -> Self {
        Self {
            size: size.into(),
            theme: self.theme.clone(),
            #[cfg(feature = "syntax")]
            highlighter: self.highlighter.clone(),
        }
    }

    /// Clone this context with the width replaced, preserving height and theme.
    pub fn with_width(&self, width: u16) -> Self {
        self.with_size((width, self.size.height))
    }

    /// Clone this context with the height replaced, preserving width and theme.
    pub fn with_height(&self, height: u16) -> Self {
        self.with_size((self.size.width, height))
    }

    /// Clone this context with the size shrunk by `insets` on each side.
    /// Saturates at zero on each axis.
    pub fn inset(&self, insets: Insets) -> Self {
        let width = self.size.width.saturating_sub(insets.left).saturating_sub(insets.right);
        let height = self.size.height.saturating_sub(insets.top).saturating_sub(insets.bottom);
        self.with_size((width, height))
    }
}

impl From<(u16, u16)> for Size {
    fn from((width, height): (u16, u16)) -> Self {
        Self { width, height }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn ctx(width: u16, height: u16) -> ViewContext {
        ViewContext::new_with_theme((width, height), Theme::default())
    }

    #[test]
    fn with_width_replaces_width_and_keeps_height() {
        let parent = ctx(80, 24);
        let child = parent.with_width(40);
        assert_eq!(child.size.width, 40);
        assert_eq!(child.size.height, 24);
    }

    #[test]
    fn with_width_preserves_theme_arc() {
        let parent = ctx(80, 24);
        let child = parent.with_width(40);
        assert!(Arc::ptr_eq(&parent.theme, &child.theme));
    }

    #[test]
    fn with_height_replaces_height_and_keeps_width() {
        let parent = ctx(80, 24);
        let child = parent.with_height(10);
        assert_eq!(child.size.width, 80);
        assert_eq!(child.size.height, 10);
    }

    #[test]
    fn inset_subtracts_on_all_sides() {
        let parent = ctx(80, 24);
        let child = parent.inset(Insets::new(2, 3, 1, 4));
        assert_eq!(child.size.width, 80 - 2 - 3);
        assert_eq!(child.size.height, 24 - 1 - 4);
    }

    #[test]
    fn inset_saturates_at_zero() {
        let parent = ctx(4, 4);
        let child = parent.inset(Insets::all(10));
        assert_eq!(child.size.width, 0);
        assert_eq!(child.size.height, 0);
    }

    #[test]
    fn insets_symmetric_sets_opposite_sides_equal() {
        let insets = Insets::symmetric(3, 5);
        assert_eq!(insets.left, 3);
        assert_eq!(insets.right, 3);
        assert_eq!(insets.top, 5);
        assert_eq!(insets.bottom, 5);
    }

    #[test]
    fn insets_horizontal_only_affects_left_and_right() {
        let insets = Insets::horizontal(4);
        assert_eq!(insets.left, 4);
        assert_eq!(insets.right, 4);
        assert_eq!(insets.top, 0);
        assert_eq!(insets.bottom, 0);
    }

    #[test]
    fn insets_vertical_only_affects_top_and_bottom() {
        let insets = Insets::vertical(2);
        assert_eq!(insets.left, 0);
        assert_eq!(insets.right, 0);
        assert_eq!(insets.top, 2);
        assert_eq!(insets.bottom, 2);
    }

    #[test]
    fn inset_horizontal_only_shrinks_width() {
        let parent = ctx(80, 24);
        let child = parent.inset(Insets::horizontal(2));
        assert_eq!(child.size.width, 76);
        assert_eq!(child.size.height, 24);
    }
}
