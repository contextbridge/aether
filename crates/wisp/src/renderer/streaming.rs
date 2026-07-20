use std::rc::Rc;

use crate::conversation::item_view::{content_width, indent_lines};
use crate::conversation::{ConversationItem, Revision};
use crate::view::markdown::{StableFence, render_markdown_streaming, streaming_stable_offset};
use crate::view::syntax::{CodeBlockState, SyntaxHighlighter};
use crate::theme::Theme;
use crate::view::wrap::as_u16;
use ratatui::text::Line;

use super::Renderer;
use super::cache::RenderShape;

impl Renderer {
    /// One still-streaming item's incremental render: the rows already known
    /// final, rendered once, plus the re-rendered unstable tail.
    pub(super) fn streaming_item_lines(
        &mut self,
        item: &ConversationItem,
        width: u16,
        padding: usize,
    ) -> Rc<[Line<'static>]> {
        let shape = RenderShape { width, padding: as_u16(padding), theme: self.generation() };
        let Self { theme, highlighter, stream_cache, stats, .. } = self;
        let text = item.text().unwrap_or("");
        let stable = streaming_stable_offset(text);

        if let Some(entry) = stream_cache.get(&item.id())
            && entry.revision == item.revision()
            && entry.shape == shape
        {
            return Rc::clone(&entry.lines);
        }
        let mut entry = match stream_cache.remove(&item.id()) {
            Some(entry) if entry.shape == shape && entry.offset <= stable.offset => entry,
            _ => StreamEntry::new(shape, item.revision()),
        };
        stats.item_rebuilds += 1;
        let reprocessed = (text.len() - entry.offset) as u64;

        if stable.offset > entry.offset {
            let (lines, state) =
                render_segment(entry.continuation(&text[entry.offset..stable.offset]), width, padding, theme, highlighter);
            entry.prefix.extend(lines);
            // Highlight state only means anything while the prefix ends inside
            // the fence it was produced in.
            entry.state = if stable.fence.is_some() { state } else { None };
            entry.offset = stable.offset;
            entry.fence.clone_from(&stable.fence);
        }

        let tail = &text[entry.offset..];
        let mut lines = entry.prefix.clone();
        if !tail.is_empty() {
            let (rendered, _) = render_segment(entry.continuation(tail), width, padding, theme, highlighter);
            lines.extend(rendered);
        }
        stats.markdown_bytes_parsed += reprocessed;
        entry.revision = item.revision();
        entry.lines = Rc::from(lines);
        let assembled = Rc::clone(&entry.lines);
        stream_cache.insert(item.id(), entry);
        assembled
    }
}

/// The incremental render state of one still-streaming item: how far into its
/// text the rendered-once prefix reaches, and everything needed to continue
/// rendering from there.
pub(super) struct StreamEntry {
    revision: Revision,
    shape: RenderShape,
    prefix: Vec<Line<'static>>,
    lines: Rc<[Line<'static>]>,
    offset: usize,
    fence: Option<StableFence>,
    state: Option<CodeBlockState>,
}

impl StreamEntry {
    fn new(shape: RenderShape, revision: Revision) -> Self {
        Self { revision, shape, prefix: Vec::new(), lines: Rc::from(Vec::new()), offset: 0, fence: None, state: None }
    }

    fn continuation<'a>(&'a self, segment: &'a str) -> Continuation<'a> {
        Continuation { segment, fence: self.fence.as_ref(), seed: self.fence.as_ref().and(self.state.clone()) }
    }
}

struct Continuation<'a> {
    segment: &'a str,
    fence: Option<&'a StableFence>,
    seed: Option<CodeBlockState>,
}

/// Renders one streaming segment — the stable prefix's extension or the
/// unstable tail — re-opening the fenced block the continuation starts inside
/// so it renders as code with carried highlight state.
fn render_segment(
    continuation: Continuation<'_>,
    width: u16,
    padding: usize,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> (Vec<Line<'static>>, Option<CodeBlockState>) {
    let Continuation { segment, fence, seed } = continuation;
    let source = match fence {
        Some(fence) => fence.opening_line() + segment,
        None => segment.to_string(),
    };
    let rendered = render_markdown_streaming(&source, content_width(width, padding), theme, highlighter, seed);
    (indent_lines(rendered.lines, padding), rendered.open_code_state)
}
