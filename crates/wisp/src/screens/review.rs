//! The shape every full-screen review has in common.
//!
//! The git-diff and plan-review screens are the same screen with different
//! content: a navigation list beside a scrollable document, a one-row footer,
//! and review comments anchored to document lines. The focus, the framing, and
//! the document pane's scroll-and-cursor behaviour live here, so each screen
//! decides only what its rows say and which keys it answers.

use crate::screens::annotation::{AnnotatedRows, AnnotatedRowsView};
use crate::surfaces::modal::frame::{MODAL_VERTICAL_CHROME, ModalFrame};
use crate::theme::Theme;
use crate::view::selection::{Direction, scroll_into_view, step_clamped};
use crate::view::widgets::key_hints;
use crate::view::wrap::as_u16;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

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

pub(crate) struct ShortcutGroup {
    pub(crate) title: &'static str,
    pub(crate) hints: &'static [(&'static str, &'static str)],
}

pub(crate) fn render_command_bar(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    actions: &[(&str, &str)],
    status: Option<&str>,
    show_help: bool,
) {
    let right = command_bar_right(status, show_help, theme);
    let right_width = as_u16(right.width()).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    let background = Style::new().bg(theme.sidebar_bg);
    Paragraph::new(command_bar_actions(actions, usize::from(left_area.width), theme))
        .style(background)
        .render(left_area, buf);
    Paragraph::new(right).style(background).right_aligned().render(right_area, buf);
}

pub(crate) fn render_shortcut_help(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    groups: &[ShortcutGroup],
) {
    let width = area.width.saturating_sub(4).min(72);
    let split = groups.len().div_ceil(2);
    let (left_groups, right_groups) = if width >= 64 { groups.split_at(split) } else { (groups, &[][..]) };
    let rows = shortcut_column_rows(left_groups).max(shortcut_column_rows(right_groups));
    let height = area.height.saturating_sub(2).min(as_u16(rows + usize::from(MODAL_VERTICAL_CHROME)));
    let frame = ModalFrame::new(
        "Review shortcuts",
        Some(key_hints(&[("?/Esc", "close")], theme)),
        Constraint::Length(width),
        Constraint::Length(height),
        theme,
    );
    let inner = frame.inner(area);
    (&frame).render(area, buf);

    if right_groups.is_empty() {
        Paragraph::new(shortcut_lines(left_groups, theme)).render(inner, buf);
        return;
    }

    let [left, _gap, right] = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Length(2),
        Constraint::Percentage(50),
    ])
    .areas(inner);
    Paragraph::new(shortcut_lines(left_groups, theme)).render(left, buf);
    Paragraph::new(shortcut_lines(right_groups, theme)).render(right, buf);
}

/// Splits a screen into its body and the one-row footer beneath it.
pub(crate) fn body_and_footer(area: Rect) -> (Rect, Rect) {
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    (body, footer)
}

fn shortcut_column_rows(groups: &[ShortcutGroup]) -> usize {
    groups.iter().map(|group| group.hints.len() + 1).sum::<usize>() + groups.len().saturating_sub(1)
}

fn shortcut_lines(groups: &[ShortcutGroup], theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(shortcut_column_rows(groups));
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::styled(group.title, Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)));
        lines.extend(group.hints.iter().map(|(key, description)| {
            Line::from(vec![
                Span::styled(format!("  {key:<12}"), Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD)),
                Span::styled(*description, Style::new().fg(theme.muted)),
            ])
        }));
    }
    lines
}

fn command_bar_actions(actions: &[(&str, &str)], width: usize, theme: &Theme) -> Line<'static> {
    let mut line = Line::from(Span::raw(" "));
    let mut used = 1;
    for (index, (key, description)) in actions.iter().enumerate() {
        let gap = usize::from(index > 0) * 3;
        let item_width = key.width() + description.width() + 3;
        if used + gap + item_width > width {
            break;
        }
        if gap > 0 {
            line.push_span(Span::raw("   "));
        }
        line.push_span(Span::styled(
            format!("[{key}]"),
            Style::new().fg(theme.accent).bg(theme.code_bg).add_modifier(Modifier::BOLD),
        ));
        line.push_span(Span::styled(format!(" {description}"), Style::new().fg(theme.text_secondary)));
        used += gap + item_width;
    }
    line
}

fn command_bar_right(status: Option<&str>, show_help: bool, theme: &Theme) -> Line<'static> {
    let mut line = Line::default();
    if let Some(status) = status {
        line.push_span(Span::styled(status.to_string(), Style::new().fg(theme.info)));
        if show_help {
            line.push_span(Span::raw("   "));
        }
    }
    if show_help {
        line.push_span(Span::styled("[?]", Style::new().fg(theme.accent).bg(theme.code_bg).add_modifier(Modifier::BOLD)));
        line.push_span(Span::styled(" shortcuts", Style::new().fg(theme.text_secondary)));
    }
    line.push_span(Span::raw(" "));
    line
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
