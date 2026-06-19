use super::AnchoredRows;
use crate::components::common::VerticalCursor;
use std::collections::HashMap;
use std::hash::Hash;
use tui::{Frame, Line, Style, Theme};

#[derive(Clone)]
pub(crate) struct FrameSplice {
    pub after_row: usize,
    pub frame: Frame,
}

pub(crate) fn compose_review_surface<A: Copy + Eq + Hash>(
    rows: &AnchoredRows<A>,
    cursor: &mut VerticalCursor,
    comment_splices: &[FrameSplice],
    draft_splice: Option<&FrameSplice>,
    theme: &Theme,
    body_height: usize,
) -> Frame {
    let lines = rows.lines();
    let lines_len = lines.len();
    let cursor_row = cursor.row;
    let draft_after_row = draft_splice.map(|d| d.after_row);

    let mut splices_by_row: HashMap<usize, Vec<&FrameSplice>> = HashMap::new();
    let mut trailing_splices: Vec<&FrameSplice> = Vec::new();
    let mut submitted_total = 0usize;
    let mut cursor_offset = 0usize;
    let mut submitted_up_to_draft = 0usize;

    for splice in comment_splices {
        let height = splice.frame.lines().len();
        submitted_total += height;

        if splice.after_row < cursor_row {
            cursor_offset += height;
        }
        if let Some(draft_row) = draft_after_row
            && splice.after_row <= draft_row
        {
            submitted_up_to_draft += height;
        }

        if splice.after_row >= lines_len {
            trailing_splices.push(splice);
        } else {
            splices_by_row.entry(splice.after_row).or_default().push(splice);
        }
    }

    let draft_total = draft_splice.map_or(0, |s| s.frame.lines().len());
    let total_height = lines_len + submitted_total + draft_total;

    if let Some(draft) = draft_splice
        && draft.after_row < cursor_row
    {
        cursor_offset += draft.frame.lines().len();
    }
    let cursor_visual_row = cursor_row + cursor_offset;

    let max_scroll = total_height.saturating_sub(body_height);
    cursor.scroll = cursor.scroll.min(max_scroll);
    cursor.ensure_visible(cursor_visual_row, body_height);

    if body_height > 0
        && let Some(first_at_cursor) = splices_by_row.get(&cursor_row).and_then(|v| v.first())
    {
        let comment_end = cursor_visual_row + first_at_cursor.frame.lines().len();
        if comment_end >= cursor.scroll + body_height {
            cursor.scroll = comment_end.saturating_sub(body_height - 1).min(cursor_visual_row);
        }
    }

    if let Some(draft) = draft_splice {
        let draft_end = draft.after_row + submitted_up_to_draft + draft.frame.lines().len();
        cursor.ensure_visible(draft_end, body_height);
    }
    cursor.scroll = cursor.scroll.min(max_scroll);

    let viewport_start = cursor.scroll;
    let viewport_end = viewport_start.saturating_add(body_height);
    let mut output = Vec::with_capacity(body_height);
    let mut visual_row = 0usize;

    let emit_line = |output: &mut Vec<Line>, visual_row: usize, line: &Line, highlight: bool| {
        if visual_row < viewport_start || visual_row >= viewport_end {
            return;
        }
        output.push(if highlight { apply_cursor_highlight(line, theme) } else { line.clone() });
    };

    for (src_idx, src_line) in lines.iter().enumerate() {
        emit_line(&mut output, visual_row, src_line, src_idx == cursor_row);
        visual_row += 1;

        if let Some(row_splices) = splices_by_row.get(&src_idx) {
            for splice in row_splices {
                for line in splice.frame.lines() {
                    emit_line(&mut output, visual_row, line, false);
                    visual_row += 1;
                }
            }
        }

        if let Some(draft) = draft_splice
            && draft.after_row == src_idx
        {
            for line in draft.frame.lines() {
                emit_line(&mut output, visual_row, line, false);
                visual_row += 1;
            }
        }

        if visual_row >= viewport_end {
            break;
        }
    }

    for splice in &trailing_splices {
        for line in splice.frame.lines() {
            emit_line(&mut output, visual_row, line, false);
            visual_row += 1;
        }
    }
    if let Some(draft) = draft_splice
        && draft.after_row >= lines_len
    {
        for line in draft.frame.lines() {
            emit_line(&mut output, visual_row, line, false);
            visual_row += 1;
        }
    }

    Frame::new(output)
}

fn apply_cursor_highlight(line: &Line, theme: &Theme) -> Line {
    let highlight_bg = theme.highlight_bg();
    let mut highlighted = Line::default();

    for span in line.spans() {
        highlighted.push_with_style(span.text(), span.style().bg_color(highlight_bg));
    }

    if line.is_empty() {
        highlighted.push_with_style(" ", Style::default().bg_color(highlight_bg));
    }

    highlighted
}

#[cfg(test)]
mod tests {
    use super::super::CommentAnchor;
    use super::*;

    fn make_rows(count: usize) -> AnchoredRows<usize> {
        let mut rows: AnchoredRows<usize> = AnchoredRows::default();
        for i in 0..count {
            rows.push_anchored_rows(CommentAnchor(i), [Line::new(format!("row{i}"))]);
        }
        rows
    }

    fn splice(after_row: usize, height: usize) -> FrameSplice {
        let lines: Vec<Line> = (0..height).map(|i| Line::new(format!("s{after_row}.{i}"))).collect();
        FrameSplice { after_row, frame: Frame::new(lines) }
    }

    fn line_texts(frame: &Frame) -> Vec<String> {
        frame.lines().iter().map(|line| line.spans().iter().map(|s| s.text().to_string()).collect()).collect()
    }

    #[test]
    fn emits_source_rows_and_splices_in_order_for_large_input() {
        let rows = make_rows(50);
        let splices: Vec<FrameSplice> = (0..50).step_by(5).map(|r| splice(r, 2)).collect();
        let mut cursor = VerticalCursor { row: 0, scroll: 0 };
        let theme = Theme::default();
        let body_height = 200;

        let frame = compose_review_surface(&rows, &mut cursor, &splices, None, &theme, body_height);

        let texts = line_texts(&frame);
        assert_eq!(texts.len(), 50 + splices.len() * 2);
        assert_eq!(texts[0], "row0");
        assert_eq!(texts[1], "s0.0");
        assert_eq!(texts[2], "s0.1");
        assert_eq!(texts[3], "row1");
    }

    #[test]
    fn cursor_highlight_applies_to_cursor_row() {
        let rows = make_rows(4);
        let mut cursor = VerticalCursor { row: 2, scroll: 0 };
        let theme = Theme::default();

        let frame = compose_review_surface(&rows, &mut cursor, &[], None, &theme, 10);

        assert_eq!(frame.lines().len(), 4);
    }

    #[test]
    fn trailing_splice_renders_after_all_source_rows() {
        let rows = make_rows(3);
        let splices = vec![splice(100, 1)];
        let mut cursor = VerticalCursor { row: 0, scroll: 0 };
        let theme = Theme::default();

        let frame = compose_review_surface(&rows, &mut cursor, &splices, None, &theme, 20);

        let texts = line_texts(&frame);
        assert_eq!(texts, vec!["row0", "row1", "row2", "s100.0"]);
    }

    #[test]
    fn draft_after_last_row_renders_at_end() {
        let rows = make_rows(2);
        let draft = splice(5, 2);
        let mut cursor = VerticalCursor { row: 0, scroll: 0 };
        let theme = Theme::default();

        let frame = compose_review_surface(&rows, &mut cursor, &[], Some(&draft), &theme, 20);

        let texts = line_texts(&frame);
        assert_eq!(texts, vec!["row0", "row1", "s5.0", "s5.1"]);
    }
}
