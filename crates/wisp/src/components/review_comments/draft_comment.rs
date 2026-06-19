use super::comment_box::{CommentBox, DRAFT_TEXT_COL_START, wrap_text};
use super::{AnchoredRows, CommentAnchor, FrameSplice};
use std::hash::Hash;
use tui::{Component, Cursor, Event, Frame, KeyCode, ViewContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraftComment<A> {
    pub anchor: CommentAnchor<A>,
    pub text: String,
    pub cursor_position: usize,
}

pub(crate) enum DraftCommentEdit<A> {
    Noop,
    Cancelled,
    Submitted { anchor: CommentAnchor<A>, text: String },
}

impl<A: Copy> DraftComment<A> {
    pub(crate) fn new(anchor: CommentAnchor<A>) -> Self {
        Self { anchor, text: String::new(), cursor_position: 0 }
    }
}

impl<A: Copy> Component for DraftComment<A> {
    type Message = DraftCommentEdit<A>;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return None;
        };

        let edit = match key.code {
            KeyCode::Esc => DraftCommentEdit::Cancelled,
            KeyCode::Enter => {
                if self.text.trim().is_empty() {
                    DraftCommentEdit::Cancelled
                } else {
                    DraftCommentEdit::Submitted { anchor: self.anchor, text: self.text.clone() }
                }
            }
            KeyCode::Char(ch) => {
                let byte_pos = char_to_byte_pos(&self.text, self.cursor_position);
                self.text.insert(byte_pos, ch);
                self.cursor_position += 1;
                DraftCommentEdit::Noop
            }
            KeyCode::Backspace => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                    let byte_pos = char_to_byte_pos(&self.text, self.cursor_position);
                    self.text.remove(byte_pos);
                }
                DraftCommentEdit::Noop
            }
            KeyCode::Left => {
                self.cursor_position = self.cursor_position.saturating_sub(1);
                DraftCommentEdit::Noop
            }
            KeyCode::Right => {
                let max = self.text.chars().count();
                self.cursor_position = (self.cursor_position + 1).min(max);
                DraftCommentEdit::Noop
            }
            _ => DraftCommentEdit::Noop,
        };

        match edit {
            DraftCommentEdit::Noop => Some(vec![]),
            edit => Some(vec![edit]),
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let text = if self.text.is_empty() { " " } else { &self.text };
        let lines = CommentBox { text }.render(ctx).into_lines();
        let inner_width = usize::from(ctx.size.width).saturating_sub(DRAFT_TEXT_COL_START);
        let (cursor_row, cursor_col_offset) = cursor_row_col(text, self.cursor_position, inner_width);
        let cursor_row_offset = 1 + cursor_row;
        let cursor_col = (DRAFT_TEXT_COL_START + cursor_col_offset).min(usize::from(ctx.size.width).saturating_sub(1));

        Frame::new(lines).with_cursor(Cursor::visible(cursor_row_offset, cursor_col))
    }
}

impl<A: Copy + Eq + Hash> DraftComment<A> {
    pub(crate) fn splice_into(&self, surface: &AnchoredRows<A>, ctx: &ViewContext) -> Option<FrameSplice> {
        let end_row = surface.end_row_for_anchor(self.anchor)?;

        let mut draft = self.clone();
        Some(FrameSplice { after_row: end_row, frame: draft.render(ctx) })
    }
}

fn char_to_byte_pos(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map_or(text.len(), |(index, _)| index)
}

fn cursor_row_col(text: &str, cursor_position: usize, max_width: usize) -> (usize, usize) {
    if max_width == 0 {
        return (0, 0);
    }

    let canonical = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let wrapped = wrap_text(if canonical.is_empty() { " " } else { &canonical }, max_width);
    let cursor = cursor_position.min(canonical.chars().count());

    let mut consumed = 0usize;
    for (row_idx, line) in wrapped.iter().enumerate() {
        let line_len = line.chars().count();
        if cursor <= consumed + line_len {
            return (row_idx, cursor.saturating_sub(consumed));
        }
        consumed += line_len + 1;
    }

    wrapped.last().map_or((0, 0), |last| (wrapped.len().saturating_sub(1), last.chars().count()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{KeyEvent, KeyModifiers, Line};

    fn key_event(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[tokio::test]
    async fn submitting_empty_text_cancels_draft() {
        let mut draft = DraftComment::new(CommentAnchor(3));

        let outcome = draft.on_event(&key_event(KeyCode::Enter)).await.unwrap();
        assert!(matches!(outcome.as_slice(), [DraftCommentEdit::Cancelled]));
    }

    #[tokio::test]
    async fn inserting_and_submitting_returns_comment() {
        let mut draft = DraftComment::new(CommentAnchor(5));
        draft.on_event(&key_event(KeyCode::Char('h'))).await;
        draft.on_event(&key_event(KeyCode::Char('i'))).await;

        let outcome = draft.on_event(&key_event(KeyCode::Enter)).await.unwrap();
        assert!(matches!(outcome.as_slice(), [DraftCommentEdit::Submitted { .. }]));
    }

    #[test]
    fn component_render_has_borders_and_cursor() {
        let mut draft = DraftComment { anchor: CommentAnchor(2), text: "test comment".to_string(), cursor_position: 4 };
        let ctx = ViewContext::new((60, 24));
        let rendered = draft.render(&ctx);

        assert!(rendered.lines().len() >= 3);
        assert!(rendered.lines()[0].plain_text().contains('┌'));
        assert!(rendered.lines().last().is_some_and(|row| row.plain_text().contains('└')));
        assert!(rendered.cursor().col >= DRAFT_TEXT_COL_START);
    }

    #[test]
    fn splice_into_uses_anchor_end_row() {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        rows.push_anchored_rows(CommentAnchor(4), [Line::new("row0")]);
        let draft = DraftComment { anchor: CommentAnchor(4), text: "hello".to_string(), cursor_position: 5 };
        let ctx = ViewContext::new((40, 24));

        let splice = draft.splice_into(&rows, &ctx).expect("draft should splice into anchored rows");

        assert_eq!(splice.after_row, 0);
    }

    #[test]
    fn splice_into_returns_none_for_unknown_anchor() {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        rows.push_unanchored_rows([Line::new("row0")]);
        let draft = DraftComment { anchor: CommentAnchor(4), text: "hello".to_string(), cursor_position: 5 };
        let ctx = ViewContext::new((40, 24));

        assert!(draft.splice_into(&rows, &ctx).is_none());
    }
}
