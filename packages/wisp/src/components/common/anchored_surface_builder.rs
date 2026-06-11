use crate::components::review_comments::{AnchoredRows, CommentAnchor};
use std::hash::Hash;
use tui::Line;

pub(crate) struct AnchoredSurfaceBuilder<A> {
    rows: AnchoredRows<A>,
}

impl<A: Copy + Eq + Hash> AnchoredSurfaceBuilder<A> {
    pub(crate) fn new() -> Self {
        Self { rows: AnchoredRows::default() }
    }

    pub(crate) fn push_anchored_wrapped(
        &mut self,
        anchor: CommentAnchor<A>,
        content: Line,
        width: u16,
        first_head: &Line,
        continuation_head: &Line,
    ) {
        let wrapped = tui::wrap_with_gutter(content, width, first_head, continuation_head).into_lines();
        self.rows.push_anchored_rows(anchor, wrapped);
    }

    pub(crate) fn push_raw_unanchored_rows(&mut self, rows: impl IntoIterator<Item = Line>) {
        self.rows.push_unanchored_rows(rows);
    }

    pub(crate) fn push_raw_anchored_rows(&mut self, anchor: CommentAnchor<A>, rows: impl IntoIterator<Item = Line>) {
        self.rows.push_anchored_rows(anchor, rows);
    }

    pub(crate) fn finish(self) -> AnchoredRows<A> {
        self.rows
    }
}

impl<A: Copy + Eq + Hash> Default for AnchoredSurfaceBuilder<A> {
    fn default() -> Self {
        Self::new()
    }
}
