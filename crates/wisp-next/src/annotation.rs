//! Comments a reviewer attaches to a line of a document, the one being typed,
//! and the document they are woven into.
//!
//! Both review screens — the git diff and the plan review — anchor comments to
//! a line, edit them the same way, and scroll the woven result, so all of that
//! lives here and each screen only decides how its own rows and boxes are drawn.

use crate::edit_buffer::{EditBuffer, apply_edit_key};
use crate::theme::Theme;
use crate::widgets::render_vertical_scrollbar;
use crate::wrap::rows as rows_u16;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Widget;

/// A comment being typed, anchored to wherever `A` points.
pub struct Draft<A> {
    pub anchor: A,
    pub buffer: EditBuffer,
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
struct Row<A> {
    line: Line<'static>,
    /// The source line this row belongs to, when it belongs to one.
    anchor: Option<A>,
    /// Whether the cursor may rest on this row.
    selectable: bool,
}

impl<A> Default for AnnotatedRows<A> {
    fn default() -> Self {
        Self { rows: Vec::new(), draft_cursor: None }
    }
}

impl<A: Copy + PartialEq> AnnotatedRows<A> {
    /// Columns left for content once [`AnnotatedRows::render`] reserves the
    /// scrollbar track. Rows are built to this width.
    pub fn content_width(area_width: u16) -> u16 {
        area_width.saturating_sub(SCROLLBAR_WIDTH)
    }

    /// A source row the cursor can rest on and annotations can hang from.
    pub fn push(&mut self, line: Line<'static>, anchor: A) {
        self.rows.push(Row { line, anchor: Some(anchor), selectable: true });
    }

    /// A source row annotations can hang from, but the cursor skips over.
    pub fn push_anchored(&mut self, line: Line<'static>, anchor: A) {
        self.rows.push(Row { line, anchor: Some(anchor), selectable: false });
    }

    /// A row that is neither, such as a placeholder while a file loads.
    pub fn push_inert(&mut self, line: Line<'static>) {
        self.rows.push(Row { line, anchor: None, selectable: false });
    }

    /// Annotation rows beneath the source row most recently pushed.
    pub fn push_annotation(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        self.rows.extend(lines.into_iter().map(|line| Row { line, anchor: None, selectable: false }));
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

    /// Draws the rows visible from `offset`, painting `cursor` in the cursor
    /// colours and reserving the rightmost column for the scrollbar track.
    pub fn render(&self, area: Rect, buf: &mut Buffer, offset: usize, cursor: Option<usize>, theme: &Theme) {
        let [body, track] = Layout::horizontal([Constraint::Min(0), Constraint::Length(SCROLLBAR_WIDTH)]).areas(area);
        for (index, row) in self.rows.iter().skip(offset).take(usize::from(body.height)).enumerate() {
            let row_area = Rect { y: body.y + rows_u16(index), height: 1, ..body };
            (&row.line).render(row_area, buf);
            if cursor == Some(offset + index) {
                paint_cursor_row(row_area, buf, theme);
            }
        }
        render_vertical_scrollbar(track, buf, self.rows.len(), offset);
    }
}

const SCROLLBAR_WIDTH: u16 = 1;

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
        draft.on_paste(text);
    }
}

/// Identity of a draft's rendered box: what it hangs from, plus the text and
/// cursor that were drawn. A cached render keyed on this redraws on every
/// keystroke, which is exactly what a box being typed into needs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DraftKey<A> {
    anchor: A,
    text_len: usize,
    cursor: usize,
}

/// The key for `draft`, if one is open.
pub fn draft_key<A: Copy>(draft: Option<&Draft<A>>) -> Option<DraftKey<A>> {
    draft.map(|draft| DraftKey {
        anchor: draft.anchor,
        text_len: draft.buffer.text().len(),
        cursor: draft.buffer.cursor(),
    })
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

    fn on_paste(&mut self, text: &str) {
        self.buffer.insert_paste(text);
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
