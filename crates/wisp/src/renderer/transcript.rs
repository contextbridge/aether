use std::rc::Rc;

use crate::app::App;
use crate::conversation::item_view::{ContentKind, content_kind, item_lines};
use crate::conversation::{ConversationContent, ConversationItem, ItemState};
use crate::view::wrap::as_u16;
use ratatui::text::Line;

use super::Renderer;
use super::cache::{RenderKey, RenderShape};
use super::history::streams_into_history;
use super::stats::Lap;

impl Renderer {
    /// The rendered rows native scrollback does not hold yet, from the commit
    /// point to the end of the conversation.
    pub(super) fn live_lines(&mut self, app: &App, width: u16) -> Vec<Line<'static>> {
        let lines = self.live_lines_inner(app, width);
        self.stats.max_live_rows = self.stats.max_live_rows.max(lines.len());
        lines
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
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut previous = previous_kind;
        for item in items {
            let kind = content_kind(item);
            if previous.is_some_and(|value| value != kind) {
                lines.push(Line::default());
            }
            lines.extend(self.cached_item_lines(item, width, padding, spinner_tick).iter().cloned());
            previous = Some(kind);
        }
        lines
    }

    fn live_lines_inner(&mut self, app: &App, width: u16) -> Vec<Line<'static>> {
        let items = app.conversation_items();
        let commit = self.native_history.commit;
        let Some(item) = items.get(commit.item_index) else {
            return Vec::new();
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
            self.lines(std::slice::from_ref(item), previous_kind, item_width, item_padding, app.spinner_tick());
        lines.drain(..commit.rows.min(lines.len()));
        lines.extend(self.lines(
            &items[commit.item_index + 1..],
            Some(content_kind(item)),
            width,
            app.content_padding(),
            app.spinner_tick(),
        ));
        lines
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
        if streams_into_history(item) && item.is_open() {
            return self.streaming_item_lines(item, width, padding);
        }
        let shape = RenderShape { width, padding: as_u16(padding), theme: self.generation() };
        let Self { theme, highlighter, render_cache, stats, stream_cache, .. } = self;
        if !stream_cache.is_empty() && item.state() == ItemState::Sealed {
            stream_cache.remove(&item.id());
        }
        let animated = matches!(item.content(), ConversationContent::Tool(_)) && item.is_open();
        let key = RenderKey {
            item_id: item.id(),
            revision: item.revision(),
            shape,
            spinner: animated.then_some(spinner_tick),
        };
        let lap = Lap::start();
        let (lines, built) = render_cache
            .get_or_insert_with(key, || Rc::from(item_lines(item, width, padding, spinner_tick, theme, highlighter)));
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
