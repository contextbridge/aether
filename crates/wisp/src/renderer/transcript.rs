use std::rc::Rc;

use crate::app::App;
use crate::conversation::item_view::{ContentKind, content_kind, content_width, item_lines};
use crate::conversation::tool_calls::ToolStatus;
use crate::conversation::{ConversationContent, ConversationItem};
use crate::view::wrap::as_u16;
use clankerdiff_ratatui::MarkdownStreamError;
use ratatui::text::Line;

use super::Renderer;
use super::cache::{RenderKey, RenderShape};
use super::history::streams_into_history;
use super::stats::Lap;

impl Renderer {
    /// The rendered rows native scrollback does not hold yet, from the commit
    /// point to the end of the conversation.
    pub(super) fn live_lines(&mut self, app: &App, width: u16) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
        let lines = self.live_lines_inner(app, width)?;
        self.stats.max_live_rows = self.stats.max_live_rows.max(lines.len());
        Ok(lines)
    }

    /// The rows of a run of items, with a blank line between runs of different
    /// content kinds.
    pub(super) fn lines(
        &mut self,
        items: &[ConversationItem],
        previous_kind: Option<ContentKind>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
        let mut lines = Vec::new();
        let mut previous = previous_kind;
        for item in items {
            lines.extend(self.item_suffix(item, previous, width, padding, spinner_tick, 0)?);
            previous = Some(content_kind(item));
        }
        Ok(lines)
    }

    fn live_lines_inner(&mut self, app: &App, width: u16) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
        let items = app.conversation_items();
        let commit = self.native_history.commit;
        let Some(item) = items.get(commit.item_index) else {
            return Ok(Vec::new());
        };
        let previous_kind = items.get(commit.item_index.wrapping_sub(1)).map(content_kind);
        if commit.rows == 0 {
            return self.lines(
                &items[commit.item_index..],
                previous_kind,
                width,
                app.content_padding(),
                app.spinner_tick(),
            );
        }

        let (item_width, item_padding) = commit.dimensions(width, app.content_padding());
        let mut lines =
            self.item_suffix(item, previous_kind, item_width, item_padding, app.spinner_tick(), commit.rows)?;
        lines.extend(self.lines(
            &items[commit.item_index + 1..],
            Some(content_kind(item)),
            width,
            app.content_padding(),
            app.spinner_tick(),
        )?);
        Ok(lines)
    }

    pub(super) fn item_suffix(
        &mut self,
        item: &ConversationItem,
        previous: Option<ContentKind>,
        width: u16,
        padding: usize,
        spinner: usize,
        skip: usize,
    ) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
        let separator = usize::from(previous.is_some_and(|kind| kind != content_kind(item)));
        let mut lines = Vec::new();
        if separator > skip {
            lines.push(Line::default());
        }
        let skip = skip.saturating_sub(separator);
        if streams_into_history(item) {
            lines.extend(self.streaming_item_lines(item, width, padding, skip)?);
        } else {
            lines.extend(self.cached_item_lines(item, width, padding, spinner).iter().skip(skip).cloned());
        }
        Ok(lines)
    }

    /// One item's rendered rows, served from the per-frame cache whenever its
    /// rendering is a pure function of its content. A streaming item renders
    /// incrementally instead; an open tool call re-renders when the spinner it
    /// shows moves, and is a cache hit for input that moves neither it nor the
    /// tool.
    fn cached_item_lines(
        &mut self,
        item: &ConversationItem,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Rc<[Line<'static>]> {
        let shape = RenderShape { width, padding: as_u16(padding), theme: self.generation() };
        let Self { theme, highlighter, render_cache, preview_cache, stats, .. } = self;
        let animated = matches!(item.content(), ConversationContent::Tool(_)) && item.is_open();
        let key = RenderKey {
            item_id: item.id(),
            revision: item.revision(),
            shape,
            spinner: animated.then_some(spinner_tick),
        };
        let lap = Lap::start();
        let (lines, built) = render_cache.get_or_insert_with(key, || {
            let preview = if let ConversationContent::Tool(tool) = item.content()
                && matches!(tool.status, ToolStatus::Success)
                && let Some(file) = &tool.diff
            {
                let entry = preview_cache
                    .entry(item.id())
                    .or_insert_with(|| (item.revision(), clankerdiff_ratatui::DiffPreviewState::new((**file).clone())));
                if entry.0 != item.revision() {
                    entry.1.set_file((**file).clone());
                    entry.0 = item.revision();
                }
                Some(entry.1.render(
                    content_width(width, padding),
                    theme.review(),
                    &mut highlighter.inner,
                    clankerdiff_ratatui::DiffPreviewOptions::default(),
                ))
            } else {
                None
            };
            Rc::from(item_lines(item, width, padding, spinner_tick, theme, highlighter, preview.as_deref()))
        });
        if !built {
            return lines;
        }
        stats.item_rebuilds += 1;
        stats.ns_item_rebuild += lap.ns();
        if let ConversationContent::Assistant(text) = item.content() {
            stats.markdown_bytes_parsed += text.text.len() as u64;
        }
        lines
    }
}
