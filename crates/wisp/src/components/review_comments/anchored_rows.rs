use crate::components::common::VerticalCursor;
use std::collections::HashMap;
use std::hash::Hash;
use tui::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CommentAnchor<A>(pub A);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnchoredGroup<A> {
    pub anchor: CommentAnchor<A>,
    pub start_row: usize,
    pub end_row: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct AnchoredRows<A> {
    lines: Vec<Line>,
    groups: Vec<AnchoredGroup<A>>,
    group_index_by_anchor: HashMap<CommentAnchor<A>, usize>,
}

impl<A> Default for AnchoredRows<A> {
    fn default() -> Self {
        Self { lines: Vec::new(), groups: Vec::new(), group_index_by_anchor: HashMap::new() }
    }
}

impl<A: Copy + Eq + Hash> AnchoredRows<A> {
    pub(crate) fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub(crate) fn max_row(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    #[cfg(test)]
    pub(crate) fn groups(&self) -> &[AnchoredGroup<A>] {
        &self.groups
    }

    pub(crate) fn push_unanchored_rows(&mut self, rows: impl IntoIterator<Item = Line>) {
        self.lines.extend(rows);
    }

    pub(crate) fn push_anchored_rows(&mut self, anchor: CommentAnchor<A>, rows: impl IntoIterator<Item = Line>) {
        let rows: Vec<_> = rows.into_iter().collect();
        if rows.is_empty() {
            return;
        }

        let start_row = self.lines.len();
        self.lines.extend(rows);
        let end_row = self.lines.len() - 1;

        if let Some(index) = self.group_index_by_anchor.get(&anchor).copied() {
            let expected_index = self.groups.len().saturating_sub(1);
            let block = self.groups.get_mut(index).expect("anchor group index should be valid");
            assert_eq!(index, expected_index, "anchored rows for the same anchor must be appended contiguously");
            assert_eq!(block.end_row + 1, start_row, "anchored rows for the same anchor must be appended contiguously");
            block.end_row = end_row;
            return;
        }

        let index = self.groups.len();
        self.groups.push(AnchoredGroup { anchor, start_row, end_row });
        self.group_index_by_anchor.insert(anchor, index);
    }

    pub(crate) fn anchor_at_or_before(&self, row: usize) -> Option<CommentAnchor<A>> {
        if self.lines.is_empty() || self.groups.is_empty() {
            return None;
        }

        let capped = row.min(self.max_row());
        self.groups.iter().rev().find(|group| group.start_row <= capped).map(|group| group.anchor)
    }

    pub(crate) fn start_row_for_anchor(&self, anchor: CommentAnchor<A>) -> Option<usize> {
        self.group_index_by_anchor.get(&anchor).map(|index| self.groups[*index].start_row)
    }

    pub(crate) fn end_row_for_anchor(&self, anchor: CommentAnchor<A>) -> Option<usize> {
        self.group_index_by_anchor.get(&anchor).map(|index| self.groups[*index].end_row)
    }

    pub(crate) fn restore_cursor(&self, cursor: &mut VerticalCursor, anchor: Option<CommentAnchor<A>>) {
        if let Some(anchor) = anchor
            && let Some(row) = self.start_row_for_anchor(anchor)
        {
            cursor.row = row;
        } else {
            cursor.row = cursor.row.min(self.max_row());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rows() -> AnchoredRows<usize> {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        rows.push_anchored_rows(CommentAnchor(1), [Line::new("row0"), Line::new("row1")]);
        rows.push_anchored_rows(CommentAnchor(2), [Line::new("row2"), Line::new("row3")]);
        rows
    }

    #[test]
    fn anchor_at_or_before_returns_anchor_for_continuation_rows() {
        let rows = make_rows();

        assert_eq!(rows.anchor_at_or_before(0), Some(CommentAnchor(1)));
        assert_eq!(rows.anchor_at_or_before(1), Some(CommentAnchor(1)));
        assert_eq!(rows.anchor_at_or_before(2), Some(CommentAnchor(2)));
        assert_eq!(rows.anchor_at_or_before(3), Some(CommentAnchor(2)));
    }

    #[test]
    fn anchor_at_or_before_uses_previous_group_for_unanchored_gap() {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        rows.push_anchored_rows(CommentAnchor(1), [Line::new("row0")]);
        rows.push_unanchored_rows([Line::new("gap")]);
        rows.push_anchored_rows(CommentAnchor(2), [Line::new("row2")]);

        assert_eq!(rows.anchor_at_or_before(1), Some(CommentAnchor(1)));
        assert_eq!(rows.anchor_at_or_before(2), Some(CommentAnchor(2)));
    }

    #[test]
    fn start_row_for_anchor_tracks_anchor_rows() {
        let rows = make_rows();

        assert_eq!(rows.start_row_for_anchor(CommentAnchor(1)), Some(0));
        assert_eq!(rows.start_row_for_anchor(CommentAnchor(2)), Some(2));
    }

    #[test]
    fn end_row_for_anchor_tracks_last_anchor_rows() {
        let rows = make_rows();

        assert_eq!(rows.end_row_for_anchor(CommentAnchor(1)), Some(1));
        assert_eq!(rows.end_row_for_anchor(CommentAnchor(2)), Some(3));
    }

    #[test]
    fn restore_cursor_uses_start_row_when_present() {
        let rows = make_rows();
        let mut cursor = VerticalCursor { row: 9, scroll: 0 };

        rows.restore_cursor(&mut cursor, Some(CommentAnchor(2)));

        assert_eq!(cursor.row, 2);
    }

    #[test]
    fn push_rows_records_anchor_groups() {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        rows.push_unanchored_rows([Line::new("gap")]);
        rows.push_anchored_rows(CommentAnchor(7), [Line::new("a"), Line::new("b")]);

        assert_eq!(rows.groups(), &[AnchoredGroup { anchor: CommentAnchor(7), start_row: 1, end_row: 2 }]);
        assert_eq!(rows.start_row_for_anchor(CommentAnchor(7)), Some(1));
        assert_eq!(rows.end_row_for_anchor(CommentAnchor(7)), Some(2));
    }

    #[test]
    fn repeated_contiguous_anchor_pushes_extend_existing_group() {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        rows.push_anchored_rows(CommentAnchor(7), [Line::new("a")]);
        rows.push_anchored_rows(CommentAnchor(7), [Line::new("b")]);

        assert_eq!(rows.groups(), &[AnchoredGroup { anchor: CommentAnchor(7), start_row: 0, end_row: 1 }]);
        assert_eq!(rows.start_row_for_anchor(CommentAnchor(7)), Some(0));
        assert_eq!(rows.end_row_for_anchor(CommentAnchor(7)), Some(1));
    }
}
