use super::{
    AnchoredRows, CommentAnchor, CommentGroup, DraftComment, DraftCommentEdit, FrameSplice, ReviewComment,
    compose_review_surface,
};
use crate::components::common::VerticalCursor;
use std::collections::HashMap;
use std::hash::Hash;
use tui::{Component, Event, Frame, KeyCode, KeyEvent, KeyModifiers, ViewContext};

pub(crate) struct ReviewSurface<A: Copy + Eq + Hash> {
    cursor: VerticalCursor,
    draft: Option<DraftComment<A>>,
}

pub(crate) enum ReviewSurfaceEvent<A> {
    CommentSubmitted { anchor: CommentAnchor<A>, text: String },
}

pub(crate) enum KeyOutcome<A> {
    Consumed,
    PassThrough,
    Event(ReviewSurfaceEvent<A>),
}

#[derive(Clone, Copy)]
pub(crate) enum Navigation<'a, A: Copy + Eq + Hash> {
    RowStep { page_size: usize },
    AnchorStep { anchors: &'a AnchorIndex<A>, page_size: usize },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AnchorIndex<A> {
    anchors: Vec<CommentAnchor<A>>,
    index_by_anchor: HashMap<CommentAnchor<A>, usize>,
}

impl<A: Copy + Eq + Hash> AnchorIndex<A> {
    pub(crate) fn push(&mut self, anchor: CommentAnchor<A>) {
        self.index_by_anchor.entry(anchor).or_insert_with(|| {
            self.anchors.push(anchor);
            self.anchors.len() - 1
        });
    }

    pub(crate) fn as_slice(&self) -> &[CommentAnchor<A>] {
        &self.anchors
    }

    pub(crate) fn index_of(&self, anchor: CommentAnchor<A>) -> Option<usize> {
        self.index_by_anchor.get(&anchor).copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.anchors.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }
}

impl<A: Copy + Eq + Hash> ReviewSurface<A> {
    pub fn new() -> Self {
        Self { cursor: VerticalCursor::new(), draft: None }
    }

    pub fn cursor(&self) -> &VerticalCursor {
        &self.cursor
    }

    pub fn cursor_mut(&mut self) -> &mut VerticalCursor {
        &mut self.cursor
    }

    pub fn is_in_comment_mode(&self) -> bool {
        self.draft.is_some()
    }

    pub fn current_anchor(&self, rows: &AnchoredRows<A>) -> Option<CommentAnchor<A>> {
        rows.anchor_at_or_before(self.cursor.row)
    }

    pub fn restore_cursor(&mut self, rows: &AnchoredRows<A>, anchor: Option<CommentAnchor<A>>) {
        rows.restore_cursor(&mut self.cursor, anchor);
    }

    pub async fn on_key(&mut self, code: KeyCode, rows: &AnchoredRows<A>, nav: Navigation<'_, A>) -> KeyOutcome<A> {
        if self.draft.is_some() {
            return self.handle_draft_key(code).await;
        }

        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                nav.move_by(1, &mut self.cursor, rows);
                KeyOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                nav.move_by(-1, &mut self.cursor, rows);
                KeyOutcome::Consumed
            }
            KeyCode::Char('g') => {
                nav.move_to_start(&mut self.cursor, rows);
                KeyOutcome::Consumed
            }
            KeyCode::Char('G') => {
                nav.move_to_end(&mut self.cursor, rows);
                KeyOutcome::Consumed
            }
            KeyCode::PageDown => {
                nav.move_by(nav.page_delta(), &mut self.cursor, rows);
                KeyOutcome::Consumed
            }
            KeyCode::PageUp => {
                nav.move_by(-nav.page_delta(), &mut self.cursor, rows);
                KeyOutcome::Consumed
            }
            KeyCode::Char('c') => {
                if let Some(anchor) = rows.anchor_at_or_before(self.cursor.row) {
                    self.draft = Some(DraftComment::new(anchor));
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::PassThrough,
        }
    }

    pub fn on_mouse_scroll(&mut self, delta: isize, rows: &AnchoredRows<A>, nav: Navigation<'_, A>) {
        if self.draft.is_some() {
            return;
        }
        nav.move_by(delta, &mut self.cursor, rows);
    }

    pub fn render_body<'a>(
        &mut self,
        rows: &AnchoredRows<A>,
        submitted: impl IntoIterator<Item = &'a ReviewComment<A>>,
        ctx: &ViewContext,
        body_height: usize,
    ) -> Frame
    where
        A: 'a,
    {
        let splices = CommentGroup::splices_for(rows, submitted, ctx);
        self.render_body_with_splices(rows, &splices, ctx, body_height)
    }

    pub fn render_body_with_splices(
        &mut self,
        rows: &AnchoredRows<A>,
        submitted_splices: &[FrameSplice],
        ctx: &ViewContext,
        body_height: usize,
    ) -> Frame {
        let draft_splice = self.draft.as_ref().and_then(|draft| draft.splice_into(rows, ctx));
        compose_review_surface(
            rows,
            &mut self.cursor,
            submitted_splices,
            draft_splice.as_ref(),
            &ctx.theme,
            body_height,
        )
    }

    async fn handle_draft_key(&mut self, code: KeyCode) -> KeyOutcome<A> {
        let Some(draft) = self.draft.as_mut() else {
            return KeyOutcome::Consumed;
        };
        let event = Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        let edit = draft.on_event(&event).await.and_then(|mut edits| edits.pop()).unwrap_or(DraftCommentEdit::Noop);
        match edit {
            DraftCommentEdit::Submitted { anchor, text } => {
                self.draft = None;
                KeyOutcome::Event(ReviewSurfaceEvent::CommentSubmitted { anchor, text })
            }
            DraftCommentEdit::Cancelled => {
                self.draft = None;
                KeyOutcome::Consumed
            }
            DraftCommentEdit::Noop => KeyOutcome::Consumed,
        }
    }
}

impl<A: Copy + Eq + Hash> Default for ReviewSurface<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Copy + Eq + Hash> Navigation<'_, A> {
    fn page_delta(&self) -> isize {
        let size = match self {
            Self::RowStep { page_size } | Self::AnchorStep { page_size, .. } => *page_size,
        };
        isize::try_from(size).unwrap_or(isize::MAX)
    }

    fn move_by(&self, delta: isize, cursor: &mut VerticalCursor, rows: &AnchoredRows<A>) -> bool {
        match self {
            Self::RowStep { .. } => cursor.move_by(delta, rows.max_row()),
            Self::AnchorStep { anchors, .. } => anchor_step_by(delta, cursor, rows, anchors),
        }
    }

    fn move_to_start(&self, cursor: &mut VerticalCursor, rows: &AnchoredRows<A>) -> bool {
        match self {
            Self::RowStep { .. } => cursor.move_to_start(),
            Self::AnchorStep { anchors, .. } => jump_to_anchor_index(0, cursor, rows, anchors),
        }
    }

    fn move_to_end(&self, cursor: &mut VerticalCursor, rows: &AnchoredRows<A>) -> bool {
        match self {
            Self::RowStep { .. } => cursor.move_to_end(rows.max_row()),
            Self::AnchorStep { anchors, .. } => {
                jump_to_anchor_index(anchors.len().saturating_sub(1), cursor, rows, anchors)
            }
        }
    }
}

fn anchor_step_by<A: Copy + Eq + Hash>(
    delta: isize,
    cursor: &mut VerticalCursor,
    rows: &AnchoredRows<A>,
    anchors: &AnchorIndex<A>,
) -> bool {
    if anchors.is_empty() {
        return false;
    }

    let current = rows.anchor_at_or_before(cursor.row);
    let current_index = current.and_then(|a| anchors.index_of(a)).unwrap_or(0);
    let max = anchors.len().saturating_sub(1);
    let new_index = if delta.is_negative() {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        (current_index + delta.unsigned_abs()).min(max)
    };
    if new_index == current_index {
        return false;
    }
    jump_to_anchor_index(new_index, cursor, rows, anchors)
}

fn jump_to_anchor_index<A: Copy + Eq + Hash>(
    index: usize,
    cursor: &mut VerticalCursor,
    rows: &AnchoredRows<A>,
    anchors: &AnchorIndex<A>,
) -> bool {
    let Some(anchor) = anchors.as_slice().get(index).copied() else {
        return false;
    };
    let Some(row) = rows.start_row_for_anchor(anchor) else {
        return false;
    };
    let changed = cursor.row != row;
    cursor.row = row;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::Line;

    fn make_rows() -> AnchoredRows<usize> {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        rows.push_anchored_rows(CommentAnchor(1), [Line::new("a")]);
        rows.push_anchored_rows(CommentAnchor(2), [Line::new("b"), Line::new("b2")]);
        rows.push_anchored_rows(CommentAnchor(3), [Line::new("c")]);
        rows
    }

    #[tokio::test]
    async fn row_step_nav_moves_one_row_at_a_time() {
        let rows = make_rows();
        let mut surface: ReviewSurface<usize> = ReviewSurface::new();

        surface.on_key(KeyCode::Char('j'), &rows, Navigation::RowStep { page_size: 20 }).await;
        assert_eq!(surface.cursor().row, 1);
    }

    #[tokio::test]
    async fn anchor_step_nav_skips_to_next_anchor() {
        let rows = make_rows();
        let mut surface: ReviewSurface<usize> = ReviewSurface::new();
        let mut anchors: AnchorIndex<usize> = AnchorIndex::default();
        anchors.push(CommentAnchor(1));
        anchors.push(CommentAnchor(2));
        anchors.push(CommentAnchor(3));

        surface.on_key(KeyCode::Char('j'), &rows, Navigation::AnchorStep { anchors: &anchors, page_size: 10 }).await;
        assert_eq!(surface.cursor().row, 1);

        surface.on_key(KeyCode::Char('j'), &rows, Navigation::AnchorStep { anchors: &anchors, page_size: 10 }).await;
        assert_eq!(surface.cursor().row, 3);
    }

    #[tokio::test]
    async fn comment_key_begins_draft_with_cursor_anchor() {
        let rows = make_rows();
        let mut surface: ReviewSurface<usize> = ReviewSurface::new();

        surface.on_key(KeyCode::Char('c'), &rows, Navigation::RowStep { page_size: 20 }).await;
        assert!(surface.is_in_comment_mode());
    }

    #[tokio::test]
    async fn draft_submit_emits_event_and_clears_draft() {
        let rows = make_rows();
        let mut surface: ReviewSurface<usize> = ReviewSurface::new();

        surface.on_key(KeyCode::Char('c'), &rows, Navigation::RowStep { page_size: 20 }).await;
        surface.on_key(KeyCode::Char('h'), &rows, Navigation::RowStep { page_size: 20 }).await;
        surface.on_key(KeyCode::Char('i'), &rows, Navigation::RowStep { page_size: 20 }).await;
        let outcome = surface.on_key(KeyCode::Enter, &rows, Navigation::RowStep { page_size: 20 }).await;

        assert!(
            matches!(outcome, KeyOutcome::Event(ReviewSurfaceEvent::CommentSubmitted { text, .. }) if text == "hi")
        );
        assert!(!surface.is_in_comment_mode());
    }

    #[tokio::test]
    async fn draft_escape_cancels_and_consumes() {
        let rows = make_rows();
        let mut surface: ReviewSurface<usize> = ReviewSurface::new();

        surface.on_key(KeyCode::Char('c'), &rows, Navigation::RowStep { page_size: 20 }).await;
        let outcome = surface.on_key(KeyCode::Esc, &rows, Navigation::RowStep { page_size: 20 }).await;

        assert!(matches!(outcome, KeyOutcome::Consumed));
        assert!(!surface.is_in_comment_mode());
    }

    #[tokio::test]
    async fn unknown_key_passes_through() {
        let rows = make_rows();
        let mut surface: ReviewSurface<usize> = ReviewSurface::new();

        let outcome = surface.on_key(KeyCode::Char('n'), &rows, Navigation::RowStep { page_size: 20 }).await;
        assert!(matches!(outcome, KeyOutcome::PassThrough));
    }
}
