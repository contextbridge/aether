//! The shape every full-screen review has in common.
//!
//! The git-diff and plan-review screens are the same screen with different
//! content: a navigation list beside a scrollable document, a one-row footer,
//! and review comments anchored to document lines. The focus, the framing, and
//! the document pane's scroll-and-cursor behaviour live here, so each screen
//! decides only what its rows say and which keys it answers.

use crate::annotation::AnnotatedRows;
use crate::selection::{Direction, scroll_into_view, step_clamped};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};

/// Rendered rows a mouse wheel notch moves the focused pane.
pub(crate) const MOUSE_SCROLL_LINES: usize = 3;

/// Which half of a review screen owns input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    /// The list beside the document: the file drawer, or the outline.
    Nav,
    /// The document under review: the patch, or the plan.
    Document,
}

/// Splits a screen into its body and the one-row footer beneath it.
pub(crate) fn body_and_footer(area: Rect) -> (Rect, Rect) {
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    (body, footer)
}

/// A pane title, brightened while that pane has focus.
pub(crate) fn focused_title(focused: bool, theme: &Theme) -> Style {
    let color = if focused { theme.accent } else { theme.text_primary };
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

/// A pane border, brightened while that pane has focus.
pub(crate) fn focused_border(focused: bool, theme: &Theme) -> Style {
    Style::new().fg(if focused { theme.accent } else { theme.muted })
}

/// A scrollable document of annotated rows, and the cursor moving through it.
///
/// Comments and drafts push source lines apart, so the scroll offset counts
/// rendered rows while the cursor names a source position. Keeping those two in
/// step — in both directions — is the whole job of this type, and is what both
/// review screens would otherwise each get slightly differently right.
pub(crate) struct DocumentPane<A> {
    rows: AnnotatedRows<A>,
    /// The source position the cursor sits on. Each screen moves this its own
    /// way, because only it knows what the positions between two lines are.
    pub(crate) cursor: A,
    /// First rendered row on screen.
    scroll: usize,
    /// Where the rows were last drawn: their height bounds scrolling, and their
    /// position hit-tests a click.
    area: Rect,
}

impl<A: Default> Default for DocumentPane<A> {
    fn default() -> Self {
        Self { rows: AnnotatedRows::default(), cursor: A::default(), scroll: 0, area: Rect::ZERO }
    }
}

impl<A: Copy + Default + PartialEq> DocumentPane<A> {
    pub(crate) fn rows(&self) -> &AnnotatedRows<A> {
        &self.rows
    }

    pub(crate) fn set_rows(&mut self, rows: AnnotatedRows<A>) {
        self.rows = rows;
    }

    pub(crate) fn scroll(&self) -> usize {
        self.scroll
    }

    /// The row the cursor currently occupies, when its source position is drawn.
    pub(crate) fn cursor_row(&self) -> Option<usize> {
        self.rows.row_of(self.cursor)
    }

    /// Scrolls by `amount` rendered rows, dragging the cursor onto whichever
    /// source line the pane now starts on.
    pub(crate) fn scroll_by(&mut self, direction: Direction, amount: usize) {
        let last_row = self.rows.len().saturating_sub(1);
        self.scroll = step_clamped(self.scroll, direction, amount, last_row);
        self.follow_scroll();
    }

    /// Scrolls the least it can to bring the cursor on screen, against the rows
    /// the last frame had.
    pub(crate) fn follow_cursor(&mut self) {
        if let Some(row) = self.cursor_row() {
            self.scroll = scroll_into_view(self.scroll, row, usize::from(self.area.height));
        }
    }

    /// Whether `position` falls inside the rows as they were last drawn.
    pub(crate) fn contains(&self, position: Position) -> bool {
        self.area.contains(position)
    }

    /// Points the cursor at the source line drawn at terminal `row`.
    pub(crate) fn focus_at(&mut self, row: u16) {
        let clicked = self.scroll + usize::from(row.saturating_sub(self.area.y));
        self.follow_row(clicked);
    }

    /// Draws the rows, scrolling the least it can to keep the cursor on screen
    /// and marking it when this pane has focus.
    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, mark_cursor: bool, theme: &Theme) {
        self.area = area;
        let cursor_row = self.cursor_row();
        if let Some(row) = cursor_row {
            self.scroll = scroll_into_view(self.scroll, row, usize::from(area.height));
        }
        self.rows.render(area, buf, self.scroll, mark_cursor.then_some(cursor_row).flatten(), theme);
    }

    fn follow_scroll(&mut self) {
        self.follow_row(self.scroll);
    }

    /// Puts the cursor on the source line owning `row`, or the nearest one above.
    fn follow_row(&mut self, row: usize) {
        if let Some(anchor) = self.rows.anchor_at_or_above(row) {
            self.cursor = anchor;
        }
    }
}
