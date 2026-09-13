use crate::app::App;
use crate::conversation::{ConversationContent, ConversationId, ConversationItem, ItemState};
use crate::error::RenderError;
use crate::view::wrap::{as_u16, wrap_line};
use clankerdiff_ratatui::MarkdownCommitError;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Widget};

use super::Renderer;
use crate::conversation::item_view::content_kind;

/// The native-scrollback cursor: how much of which conversation the terminal's
/// real scrollback already holds.
#[derive(Default)]
pub(super) struct NativeHistoryCursor {
    pub(super) conversation_id: Option<ConversationId>,
    pub(super) commit: CommitPoint,
    pub(super) committed_replacements: u64,
}

/// How much of the conversation the terminal's native scrollback already
/// holds: every item before `item_index`, plus the first `rows` rendered rows
/// of the item at `item_index`.
#[derive(Clone, Copy, Default)]
pub(super) struct CommitPoint {
    pub(super) item_index: usize,
    pub(super) rows: usize,
    /// Native history cannot reflow or be rewritten, so a partially committed
    /// item keeps rendering at the dimensions its committed rows were produced
    /// with.
    pub(super) width: u16,
    pub(super) padding: usize,
}

impl CommitPoint {
    pub(super) fn dimensions(self, width: u16, padding: usize) -> (u16, usize) {
        if self.rows > 0 { (self.width, self.padding) } else { (width, padding) }
    }
}

impl Renderer {
    pub(super) fn reconcile_history<T: Backend>(
        &mut self,
        terminal: &mut Terminal<T>,
        app: &App,
    ) -> Result<(), RenderError<T::Error>> {
        let current = committed_replacements(app.conversation_items(), self.native_history.commit);
        if current != self.native_history.committed_replacements {
            let width = terminal.size().map_err(RenderError::Backend)?.width;
            let notice = wrap_line(Line::raw("Transcript updated; earlier scrollback is superseded."), width);
            insert_history_lines(terminal, &notice, |inserted| {
                self.stats.history_rows_inserted += inserted as u64;
                Ok(())
            })?;

            self.native_history.commit = CommitPoint::default();
            self.native_history.committed_replacements = 0;
            self.render_cache.clear();
            self.stream_cache.clear();
        }
        Ok(())
    }

    /// Moves transcript rows the viewport can no longer show into the
    /// terminal's native scrollback, advancing the commit point, and returns
    /// the live rows left over for the viewport to draw.
    ///
    /// Sealed items commit whole, so an uncommitted sealed item can still
    /// reflow on resize. The open streaming item at the end commits row by row
    /// as it overflows. User messages commit whole, including optimistic echoes;
    /// a changed agent acknowledgment uses the same correction boundary as any
    /// other replacement. An open tool call redraws in place, so it and
    /// everything after it stay live.
    pub(super) fn commit_overflow<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        app: &App,
        width: u16,
        capacity: usize,
    ) -> Result<Vec<Line<'static>>, RenderError<B::Error>> {
        let items = app.conversation_items();
        let live = self.live_lines(app, width)?;
        let mut overflow = live.len().saturating_sub(capacity);
        if overflow == 0 {
            return Ok(live);
        }
        while overflow > 0 {
            let commit = self.native_history.commit;
            let Some(item) = items.get(commit.item_index) else {
                break;
            };
            let (item_width, item_padding) = commit.dimensions(width, app.content_padding());
            let previous = items.get(commit.item_index.wrapping_sub(1)).map(content_kind);
            let separator = usize::from(previous.is_some_and(|kind| kind != content_kind(item)));
            let rendered =
                self.item_suffix(item, previous, item_width, item_padding, app.spinner_tick(), commit.rows)?;
            let committed = commit.rows;
            let pending = rendered.as_slice();
            let whole = item.state() == ItemState::Sealed || matches!(item.content(), ConversationContent::User(_));
            let take = if whole {
                pending.len()
            } else if streams_into_history(item) {
                overflow.min(pending.len().saturating_sub(1))
            } else {
                break;
            };
            let mut rows = committed;
            insert_history_lines(terminal, &pending[..take], |inserted| {
                rows += inserted;
                self.stats.history_rows_inserted += inserted as u64;
                self.native_history.commit =
                    CommitPoint { item_index: commit.item_index, rows, width: item_width, padding: item_padding };
                self.acknowledge_stream_rows(item, rows.saturating_sub(separator))
            })?;
            if !whole {
                break;
            }
            self.stream_cache.remove(&item.id());
            overflow = overflow.saturating_sub(take);
            self.native_history.commit = CommitPoint { item_index: commit.item_index + 1, ..CommitPoint::default() };
        }
        self.native_history.committed_replacements = committed_replacements(items, self.native_history.commit);
        self.live_lines(app, width).map_err(RenderError::from)
    }
}

fn committed_replacements(items: &[ConversationItem], commit: CommitPoint) -> u64 {
    let committed = (commit.item_index + usize::from(commit.rows > 0)).min(items.len());
    items[..committed].iter().map(|item| item.replacement_revision().value()).sum()
}

/// Whether an item's rendered rows may enter native scrollback while it is
/// still open. Streaming text only ever appends rows, so everything but the
/// still-growing last row is final; an open tool call redraws in place
/// (spinner, status, sub-agent tree) and must stay live until sealed.
pub(super) fn streams_into_history(item: &ConversationItem) -> bool {
    matches!(item.content(), ConversationContent::Assistant(_))
}

/// The only function that writes to the terminal outside a frame draw.
fn insert_history_lines<T: Backend>(
    terminal: &mut Terminal<T>,
    lines: &[Line<'static>],
    mut acknowledge: impl FnMut(usize) -> Result<(), MarkdownCommitError>,
) -> Result<(), RenderError<T::Error>> {
    let height = terminal.size().map_err(RenderError::Backend)?.height;
    let viewport_height = terminal.get_frame().area().height;
    let batch_size = usize::from(height.saturating_sub(viewport_height).max(1));
    for chunk in lines.chunks(batch_size) {
        let inserted = chunk.len();
        let chunk = chunk.to_vec();
        terminal
            .insert_before(as_u16(inserted), move |buffer| {
                Paragraph::new(Text::from(chunk)).render(buffer.area, buffer);
            })
            .map_err(RenderError::Backend)?;
        acknowledge(inserted)?;
    }
    Ok(())
}
