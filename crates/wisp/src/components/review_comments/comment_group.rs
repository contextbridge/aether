use super::comment_box::CommentBox;
use super::{AnchoredRows, CommentAnchor, FrameSplice, ReviewComment};
use std::collections::HashMap;
use std::hash::Hash;
use tui::{Component, Frame, ViewContext};

pub(crate) struct CommentGroup<'a, A> {
    pub(crate) anchor: CommentAnchor<A>,
    comments: Vec<&'a ReviewComment<A>>,
}

impl<'a, A> CommentGroup<'a, A> {
    pub(crate) fn collect_from(comments: impl IntoIterator<Item = &'a ReviewComment<A>>) -> Vec<Self>
    where
        A: 'a + Copy + Eq + Hash,
    {
        let mut grouped: Vec<(CommentAnchor<A>, Vec<&ReviewComment<A>>)> = Vec::new();
        let mut anchor_to_grouped_index: HashMap<CommentAnchor<A>, usize> = HashMap::new();

        for comment in comments {
            if let Some(index) = anchor_to_grouped_index.get(&comment.anchor).copied() {
                grouped[index].1.push(comment);
            } else {
                let index = grouped.len();
                grouped.push((comment.anchor, vec![comment]));
                anchor_to_grouped_index.insert(comment.anchor, index);
            }
        }

        grouped.into_iter().map(|(anchor, comments)| Self::new(anchor, comments)).collect()
    }

    pub(crate) fn splices_for(
        surface: &AnchoredRows<A>,
        comments: impl IntoIterator<Item = &'a ReviewComment<A>>,
        ctx: &ViewContext,
    ) -> Vec<FrameSplice>
    where
        A: 'a + Copy + Eq + Hash,
    {
        let mut splices: Vec<FrameSplice> = Vec::new();

        for mut group in Self::collect_from(comments) {
            let Some(end_row) = surface.end_row_for_anchor(group.anchor) else {
                continue;
            };

            let frame = group.render(ctx);
            if let Some(last) = splices.last_mut()
                && last.after_row == end_row
            {
                let mut merged = last.frame.lines().to_vec();
                merged.extend(frame.into_lines());
                last.frame = Frame::new(merged);
            } else {
                splices.push(FrameSplice { after_row: end_row, frame });
            }
        }

        splices
    }

    fn new(anchor: CommentAnchor<A>, comments: Vec<&'a ReviewComment<A>>) -> Self {
        Self { anchor, comments }
    }
}

impl<A> Component for CommentGroup<'_, A> {
    type Message = ();

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let mut lines = Vec::new();

        for comment in &self.comments {
            let mut comment_box = CommentBox { text: &comment.body };
            lines.extend(comment_box.render(ctx).into_lines());
        }

        Frame::new(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::Line;

    #[test]
    fn render_comment_blocks_preserves_anchor_order() {
        let comments = [
            ReviewComment::<usize>::new(CommentAnchor(4), "alpha"),
            ReviewComment::<usize>::new(CommentAnchor(4), "beta"),
        ];
        let ctx = ViewContext::new((60, 24));

        let mut groups = CommentGroup::collect_from(comments.iter());
        assert_eq!(groups.len(), 1);

        let frame = groups[0].render(&ctx);
        assert!(frame.lines().iter().any(|row| row.plain_text().contains("alpha")));
        assert!(frame.lines().iter().any(|row| row.plain_text().contains("beta")));
    }

    #[test]
    fn splices_group_by_anchor() {
        let mut surface: AnchoredRows<usize> = AnchoredRows::default();
        surface.push_anchored_rows(CommentAnchor(3), [Line::new("row0"), Line::new("row1")]);
        let comments = [ReviewComment::new(CommentAnchor(3), "alpha"), ReviewComment::new(CommentAnchor(3), "beta")];
        let ctx = ViewContext::new((60, 24));

        let splices = CommentGroup::splices_for(&surface, comments.iter(), &ctx);

        assert_eq!(splices.len(), 1);
        assert_eq!(splices[0].after_row, 1);
        let rows = splices[0].frame.lines();
        assert!(rows.iter().any(|line| line.plain_text().contains("alpha")));
        assert!(rows.iter().any(|line| line.plain_text().contains("beta")));
    }
}
