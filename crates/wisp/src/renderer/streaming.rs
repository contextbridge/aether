use std::sync::Arc;

use crate::conversation::item_view::{content_width, indent_lines};
use crate::conversation::{ConversationItem, Revision};
use crate::view::markdown::layout_options;
use crate::view::wrap::as_u16;
use clankerdiff_markdown::MarkdownStream;
use clankerdiff_ratatui::{
    MarkdownCommitError, MarkdownRenderer, MarkdownRow, MarkdownStreamError, StreamingMarkdownPolicy,
    StreamingMarkdownState,
};
use ratatui::text::Line;

use super::Renderer;
use super::cache::RenderShape;

pub(super) struct StreamEntry {
    revision: Option<Revision>,
    shape: RenderShape,
    stream: MarkdownStream,
    state: StreamingMarkdownState,
    rows: Vec<Arc<MarkdownRow>>,
    row_revision: u64,
}

impl Renderer {
    pub(super) fn streaming_item_lines(
        &mut self,
        item: &ConversationItem,
        width: u16,
        padding: usize,
        skip: usize,
    ) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
        let shape = RenderShape { width, padding: as_u16(padding), theme: self.generation() };
        let entry = self.stream_cache.entry(item.id()).or_insert_with(|| StreamEntry::new(shape));
        if entry.revision != Some(item.revision()) || entry.shape != shape {
            let text = item.text().unwrap_or_default();
            let consumed = entry.stream.source().len();
            entry.stream.push(&text[consumed..]);
            if !item.is_open() {
                entry.stream.finish();
            }

            MarkdownRenderer::new().render_stream_layout(
                &mut entry.state,
                &entry.stream,
                layout_options(content_width(width, padding)),
                self.theme.review(),
                &mut self.highlighter.inner,
            )?;

            let update = entry.state.update_since(entry.row_revision);
            entry.rows.truncate(update.first_changed_row);
            entry.rows.extend(update.replacement.iter().cloned());
            entry.row_revision = update.revision;
            entry.revision = Some(item.revision());
            entry.shape = shape;

            let work = entry.state.take_stats();
            self.stats.item_rebuilds += 1;
            self.stats.markdown_bytes_parsed += work.parsed_bytes as u64;
            self.stats.markdown_bytes_scanned += work.scanned_bytes as u64;

            self.stats.markdown_prefix_bytes_copied += work.prefix_bytes_copied as u64;
            self.stats.markdown_rows_generated += work.rows_generated as u64;
        }
        let rows = entry.rows.iter().skip(skip).map(|row| row.line.clone()).collect::<Vec<_>>();
        self.stats.markdown_rows_materialized += rows.len() as u64;
        Ok(indent_lines(rows, padding))
    }

    pub(super) fn acknowledge_stream_rows(
        &mut self,
        item: &ConversationItem,
        rows: usize,
    ) -> Result<(), MarkdownCommitError> {
        if let Some(entry) = self.stream_cache.get_mut(&item.id()) {
            entry.state.commit_rows(entry.row_revision, rows)?;
        }
        Ok(())
    }
}

impl StreamEntry {
    fn new(shape: RenderShape) -> Self {
        Self {
            revision: None,
            shape,
            stream: MarkdownStream::new(),
            state: StreamingMarkdownState::new(StreamingMarkdownPolicy::Terminal),
            rows: Vec::new(),
            row_revision: 0,
        }
    }
}
