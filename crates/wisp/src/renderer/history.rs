use crate::app::App;
use crate::conversation::{ConversationContent, ConversationId, ConversationItem, ItemState};
use crate::view::wrap::as_u16;
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
    ) -> Result<Vec<Line<'static>>, B::Error> {
        let items = app.conversation_items();
        let live = self.live_lines(app, width);
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
            let rendered = self.lines(
                std::slice::from_ref(item),
                items.get(commit.item_index.wrapping_sub(1)).map(content_kind),
                item_width,
                item_padding,
                app.spinner_tick(),
            );
            let committed = commit.rows.min(rendered.len());
            let pending = &rendered[committed..];
            match item.state() {
                ItemState::Sealed => {
                    insert_history_lines(terminal, pending)?;
                    self.stats.history_rows_inserted += pending.len() as u64;
                    overflow = overflow.saturating_sub(pending.len());
                    self.native_history.commit = CommitPoint {
                        item_index: commit.item_index + 1,
                        ..CommitPoint::default()
                    };
                }
                ItemState::Open if streams_into_history(item) => {
                    // The still-growing last row stays live: appending text
                    // can rewrap it, and native history cannot be rewritten.
                    let take = overflow.min(pending.len().saturating_sub(1));
                    insert_history_lines(terminal, &pending[..take])?;
                    self.stats.history_rows_inserted += take as u64;
                    self.native_history.commit = CommitPoint {
                        item_index: commit.item_index,
                        rows: committed + take,
                        width: item_width,
                        padding: item_padding,
                    };
                    break;
                }
                ItemState::Open => break,
            }
        }
        Ok(self.live_lines(app, width))
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
fn insert_history_lines<B: Backend>(terminal: &mut Terminal<B>, lines: &[Line<'static>]) -> Result<(), B::Error> {
    for chunk in lines.chunks(usize::from(u16::MAX)) {
        let chunk = chunk.to_vec();
        terminal.insert_before(as_u16(chunk.len()), move |buffer| {
            Paragraph::new(Text::from(chunk)).render(buffer.area, buffer);
        })?;
    }
    Ok(())
}
