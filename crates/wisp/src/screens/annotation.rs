//! Comments a reviewer attaches to a line of a document, the one being typed,
//! and the document they are woven into.
//!
//! Both review screens — the git diff and the plan review — anchor comments to
//! a line, edit them the same way, and scroll the woven result, so all of that
//! lives here and each screen only decides how its own rows and boxes are drawn.

use crate::view::edit_buffer::{EditBuffer, apply_edit_key};
use crate::theme::Theme;
use crate::view::widgets::{SCROLLBAR_WIDTH, render_vertical_scrollbar, row_area, rows_and_track};
use crate::view::wrap::{fit_line, text_position_in_wrap, wrap_text_char};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Widget;
use std::rc::Rc;
use unicode_width::UnicodeWidthStr;

const COMMENT_BODY_PREFIX: &str = "│ > ";
const COMMENT_RIGHT_PADDING: usize = 3;

/// A comment being typed, anchored to wherever `A` points.
pub struct Draft<A> {
    pub anchor: A,
    pub buffer: EditBuffer,
}

/// Wraps a draft's text to `width` columns, with the cursor's row and column
/// within the wrapped result.
///
/// For a draft box embedded in scrolled content, where the terminal cursor has
/// to be placed by the caller rather than by a focused widget.
pub fn wrapped_with_cursor(buffer: &EditBuffer, width: usize) -> (Vec<String>, (usize, u16)) {
    (wrap_text_char(buffer.text(), width), text_position_in_wrap(&buffer.text()[..buffer.cursor()], width))
}

/// Columns of a `width`-column comment box left for the body text, after the
/// border and the `"│ > "` prefix.
pub fn comment_body_width(width: u16) -> usize {
    usize::from(width)
        .saturating_sub(COMMENT_BODY_PREFIX.width())
        .saturating_sub(COMMENT_RIGHT_PADDING)
        .max(1)
}

/// Draws a boxed annotation beneath a document line, used for both submitted
/// comments and the draft being typed.
pub fn comment_box(
    title: &str,
    body: &[String],
    border_color: ratatui::style::Color,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let surface = Style::new().bg(theme.sidebar_bg);
    let border = surface.fg(border_color);
    let body_style = surface.fg(theme.text_primary);
    let width = usize::from(width);

    let mut lines = vec![fit_line(Line::styled(title.to_string(), border), width, border)];
    lines.extend(body.iter().map(|text| {
        fit_line(
            Line::styled(format!("{COMMENT_BODY_PREFIX}{text}"), body_style),
            width,
            body_style,
        )
    }));
    lines.push(fit_line(Line::styled("└", border), width, border));
    lines
}

/// The draft's wrapped body and the terminal-cursor position within the
/// comment box [`comment_box`] draws it into.
pub fn draft_body<A>(draft: &Draft<A>, body_width: usize) -> (Vec<String>, (usize, u16)) {
    let (lines, (row, column)) = wrapped_with_cursor(&draft.buffer, body_width);
    (lines, (1 + row, u16::try_from(COMMENT_BODY_PREFIX.width()).unwrap_or(u16::MAX).saturating_add(column)))
}

/// A document laid out as rendered rows, with review annotations woven in
/// beneath the source lines they hang from.
///
/// Rows are built once per change and then drawn a window at a time, so a long
/// document costs one `Line` per rendered row rather than an off-screen buffer
/// of the whole thing.
pub struct AnnotatedRows<A> {
    rows: Vec<Row<A>>,
    draft_cursor: Option<(usize, u16)>,
}

/// One rendered row: a source line, or an annotation hanging beneath one.
///
/// The line is shared rather than owned so a caller can cache the expensive
/// half — the syntax-highlighted document — and reweave its annotations on
/// every keystroke for the cost of a pointer copy per row.
#[derive(Clone)]
pub struct Row<A> {
    line: Rc<Line<'static>>,
    /// The source line this row belongs to, when it belongs to one.
    anchor: Option<A>,
    /// Whether the cursor may rest on this row.
    selectable: bool,
}

impl<A: Copy> Row<A> {
    /// A source row the cursor can rest on and annotations can hang from.
    pub fn at(line: Line<'static>, anchor: A) -> Self {
        Self { line: Rc::new(line), anchor: Some(anchor), selectable: true }
    }

    /// A source row annotations can hang from, but the cursor skips over.
    pub fn anchored(line: Line<'static>, anchor: A) -> Self {
        Self { line: Rc::new(line), anchor: Some(anchor), selectable: false }
    }

    /// A row that is neither, such as a placeholder while a file loads.
    pub fn inert(line: Line<'static>) -> Self {
        Self { line: Rc::new(line), anchor: None, selectable: false }
    }

    /// What a comment on this row would anchor to, when it can carry one.
    pub fn anchor(&self) -> Option<A> {
        self.anchor
    }
}

impl<A> Default for AnnotatedRows<A> {
    fn default() -> Self {
        Self { rows: Vec::new(), draft_cursor: None }
    }
}

/// Visual command for a window of annotated document rows.
pub struct AnnotatedRowsView<'a, A> {
    rows: &'a AnnotatedRows<A>,
    offset: usize,
    cursor: Option<usize>,
    theme: &'a Theme,
}

impl<'a, A> AnnotatedRowsView<'a, A> {
    pub fn new(rows: &'a AnnotatedRows<A>, offset: usize, cursor: Option<usize>, theme: &'a Theme) -> Self {
        Self { rows, offset, cursor, theme }
    }
}

impl<A: Copy + PartialEq> Widget for AnnotatedRowsView<'_, A> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (body, track) = rows_and_track(area, true);
        for (index, row) in self.rows.rows.iter().skip(self.offset).enumerate() {
            let Some(row_area) = row_area(body, index) else {
                break;
            };
            row.line.as_ref().render(row_area, buf);
            if self.cursor == Some(self.offset + index) {
                paint_cursor_row(row_area, buf, self.theme);
            }
        }
        render_vertical_scrollbar(track, buf, self.rows.rows.len(), self.offset);
    }
}

impl<A: Copy + PartialEq> AnnotatedRows<A> {
    /// Columns left for content once [`AnnotatedRowsView`] reserves the
    /// scrollbar track. Rows are built to this width.
    pub fn content_width(area_width: u16) -> u16 {
        area_width.saturating_sub(SCROLLBAR_WIDTH)
    }

    /// A source row the cursor can rest on and annotations can hang from.
    pub fn push(&mut self, line: Line<'static>, anchor: A) {
        self.rows.push(Row::at(line, anchor));
    }

    /// A source row annotations can hang from, but the cursor skips over.
    pub fn push_anchored(&mut self, line: Line<'static>, anchor: A) {
        self.rows.push(Row::anchored(line, anchor));
    }

    /// A source row built elsewhere, so a caller that caches its rendered
    /// document can reweave annotations without rebuilding it.
    pub fn push_row(&mut self, row: &Row<A>) {
        self.rows.push(row.clone());
    }

    /// Annotation rows beneath the source row most recently pushed.
    pub fn push_annotation(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        self.rows.extend(lines.into_iter().map(Row::inert));
    }

    /// Like [`AnnotatedRows::push_annotation`], recording where the draft's text
    /// cursor lands. `cursor` is relative to the first line pushed.
    pub fn push_draft(&mut self, lines: Vec<Line<'static>>, cursor: (usize, u16)) {
        self.draft_cursor = Some((self.rows.len() + cursor.0, cursor.1));
        self.push_annotation(lines);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// The first row `anchor`'s source line was drawn on.
    pub fn row_of(&self, anchor: A) -> Option<usize> {
        self.rows.iter().position(|row| row.anchor == Some(anchor) && row.selectable)
    }

    /// The source line owning `row`, or the nearest one above it — what the
    /// cursor should follow when the pane is scrolled rather than stepped.
    pub fn anchor_at_or_above(&self, row: usize) -> Option<A> {
        self.rows.iter().take(row.saturating_add(1)).rev().find_map(|row| row.selectable.then_some(row.anchor?))
    }

    /// Where the draft's text cursor sits, as a row and column.
    pub fn draft_cursor(&self) -> Option<(usize, u16)> {
        self.draft_cursor
    }
}

/// Repaints a whole row in the cursor colours, including the columns past the
/// end of its text.
fn paint_cursor_row(area: Rect, buf: &mut Buffer, theme: &Theme) {
    for x in area.x..area.right() {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_bg(theme.accent);
            cell.set_fg(theme.background);
        }
    }
}

/// Applies one keystroke to the draft in `slot`, clearing it once the draft
/// ends. Returns the anchor and body of a comment that was filed.
///
/// Both review screens keep their draft in an `Option` and end it the same way,
/// so the slot bookkeeping lives here and [`DraftOutcome`] never leaves this
/// module.
pub fn apply_draft_key<A: Copy>(slot: &mut Option<Draft<A>>, key: KeyEvent) -> Option<(A, String)> {
    let draft = slot.as_mut()?;
    let anchor = draft.anchor;
    match draft.on_key(key) {
        DraftOutcome::Continue => None,
        DraftOutcome::Commit(body) => {
            *slot = None;
            Some((anchor, body))
        }
        DraftOutcome::Discard => {
            *slot = None;
            None
        }
    }
}

/// Pastes into the draft in `slot`, if one is open.
pub fn paste_into_draft<A>(slot: &mut Option<Draft<A>>, text: &str) {
    if let Some(draft) = slot.as_mut() {
        draft.buffer.insert_paste(text);
    }
}

/// What a keystroke did to a [`Draft`].
enum DraftOutcome {
    /// Still being typed.
    Continue,
    /// Abandoned, or finished empty. Either way there is nothing to file.
    Discard,
    /// Finished, with the body that was typed.
    Commit(String),
}

impl<A> Draft<A> {
    pub fn new(anchor: A) -> Self {
        Self { anchor, buffer: EditBuffer::default() }
    }

    /// Applies one keystroke. Enter files whatever was typed, Esc abandons it,
    /// and everything else goes to the shared editing keys.
    fn on_key(&mut self, key: KeyEvent) -> DraftOutcome {
        match key.code {
            KeyCode::Esc => DraftOutcome::Discard,
            KeyCode::Enter => {
                let body = self.buffer.take();
                if body.trim().is_empty() { DraftOutcome::Discard } else { DraftOutcome::Commit(body) }
            }
            _ => {
                apply_edit_key(&mut self.buffer, key);
                DraftOutcome::Continue
            }
        }
    }
}
