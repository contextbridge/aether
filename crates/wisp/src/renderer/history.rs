use crate::app::App;
use crate::conversation::{ConversationContent, ConversationId, ConversationItem, ItemState};
use crate::error::RenderError;
use crate::view::wrap::as_u16;
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
    /// Moves transcript rows the viewport can no longer show into the
    /// terminal's native scrollback, advancing the commit point, and returns
    /// the live rows left over for the viewport to draw.
    ///
    /// Sealed items commit whole, so an uncommitted sealed item can still
    /// reflow on resize. The open streaming item at the end commits row by row
    /// as it overflows; an open tool call redraws in place, so it and
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
            let take = match item.state() {
                ItemState::Sealed => pending.len(),
                ItemState::Open if streams_into_history(item) => overflow.min(pending.len().saturating_sub(1)),
                ItemState::Open => break,
            };
            let mut rows = committed;
            insert_history_lines(terminal, &pending[..take], |inserted| {
                rows += inserted;
                self.stats.history_rows_inserted += inserted as u64;
                self.native_history.commit =
                    CommitPoint { item_index: commit.item_index, rows, width: item_width, padding: item_padding };
                self.acknowledge_stream_rows(item, rows.saturating_sub(separator))
            })?;
            if item.state() == ItemState::Open {
                break;
            }
            self.stream_cache.remove(&item.id());
            self.preview_cache.remove(&item.id());
            overflow = overflow.saturating_sub(take);
            self.native_history.commit = CommitPoint { item_index: commit.item_index + 1, ..CommitPoint::default() };
        }
        self.live_lines(app, width).map_err(RenderError::from)
    }
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
