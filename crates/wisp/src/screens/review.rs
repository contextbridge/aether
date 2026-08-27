//! The shape every full-screen review has in common.
//!
//! The git-diff and plan-review screens are the same screen with different
//! content: a navigation list beside a scrollable document, a one-row footer,
//! and review comments anchored to document lines. The focus, the framing, and
//! the document pane's scroll-and-cursor behaviour live here, so each screen
//! decides only what its rows say and which keys it answers.

use crate::screens::annotation::{AnnotatedRows, AnnotatedRowsView};
use crate::view::selection::{Direction, scroll_into_view, step_clamped};
use crate::theme::Theme;
use crate::view::wrap::as_u16;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

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
    /// Draws `rows` and adopts them, so scrolling, cursor following, and click
    /// hit-testing all work against exactly what was last on screen.
    pub(crate) fn render_rows(
        &mut self,
        rows: AnnotatedRows<A>,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        mark_cursor: bool,
    ) {
        self.rows = rows;
        self.area = area;
        let cursor = self.rows.row_of(self.cursor);
        let focused_row = self.rows.draft_cursor().map(|(row, _)| row).or(cursor);
        self.scroll =
            focused_row.map_or(self.scroll, |row| scroll_into_view(self.scroll, row, usize::from(area.height)));
        AnnotatedRowsView::new(&self.rows, self.scroll, mark_cursor.then_some(cursor).flatten(), theme)
            .render(area, buf);
    }

    /// Terminal-cursor position for the draft comment being typed, when its
    /// insertion point is on screen.
    pub(crate) fn draft_cursor_position(&self) -> Option<Position> {
        let (draft_row, draft_col) = self.rows.draft_cursor()?;
        if draft_row < self.scroll || draft_row >= self.scroll + usize::from(self.area.height) {
            return None;
        }
        let row = self.area.y + as_u16(draft_row - self.scroll);
        let column = (self.area.x + draft_col).min(self.area.right().saturating_sub(1));
        Some(Position::new(column, row))
    }

    /// Scrolls by `amount` rendered rows, dragging the cursor onto whichever
    /// source line the pane now starts on.
    pub(crate) fn scroll_by(&mut self, direction: Direction, amount: usize) {
        let last_row = self.rows.len().saturating_sub(1);
        self.scroll = step_clamped(self.scroll, direction, amount, last_row);
        self.follow_row(self.scroll);
    }

    /// Scrolls the least it can to bring the cursor on screen, against the rows
    /// the last frame had.
    pub(crate) fn follow_cursor(&mut self) {
        if let Some(row) = self.rows.row_of(self.cursor) {
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

    /// Puts the cursor on the source line owning `row`, or the nearest one above.
    fn follow_row(&mut self, row: usize) {
        if let Some(anchor) = self.rows.anchor_at_or_above(row) {
            self.cursor = anchor;
        }
    }
}
