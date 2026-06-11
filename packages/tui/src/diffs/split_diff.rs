use crate::diffs::diff::highlight_diff;
use crate::line::Line;
use crate::rendering::frame::{FitOptions, Frame, FramePart};
use crate::rendering::gutter::digit_count;
use crate::rendering::render_context::ViewContext;
use crate::span::Span;
use crate::style::Style;
use crossterm::style::Color;

use crate::{DiffPreview, DiffTag, SplitDiffEntry};

const MAX_DIFF_LINES: usize = 20;
pub const MIN_SPLIT_WIDTH: u16 = 80;
pub const MIN_GUTTER_WIDTH: usize = 3;
pub const SEPARATOR: &str = " ";
pub const SEPARATOR_WIDTH: usize = 1;
const SEPARATOR_WIDTH_U16: u16 = 1;
const LEFT_INNER_PAD: usize = 1;
pub const WRAP_MARKER: &str = "↪";

/// Which pane a [`render_entry`] call is rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Whether changed rows tint the line-number gutter with their diff color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GutterTint {
    Neutral,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitLayoutDimensions {
    pub gutter_width: usize,
    pub left_content_width: usize,
    pub right_content_width: usize,
    pub left_panel_width: u16,
    pub right_panel_width: u16,
}

impl SplitLayoutDimensions {
    pub fn new(total_width: usize, max_line_no: usize) -> Self {
        let gutter_width = gutter_width_for_max_line(max_line_no);
        let fixed_overhead = gutter_width * 2 + SEPARATOR_WIDTH + LEFT_INNER_PAD;
        let usable = total_width.saturating_sub(fixed_overhead);
        let left_content_width = usable / 2;
        let right_content_width = usable - left_content_width;
        Self {
            gutter_width,
            left_content_width,
            right_content_width,
            left_panel_width: usize_to_u16_saturating(gutter_width + left_content_width + LEFT_INNER_PAD),
            right_panel_width: usize_to_u16_saturating(gutter_width + right_content_width),
        }
    }

    pub fn for_preview(preview: &DiffPreview, total_width: usize) -> Self {
        Self::new(total_width, max_line_no_for_preview(preview))
    }
}

fn gutter_width_for_max_line(max_line_no: usize) -> usize {
    (digit_count(max_line_no) + 1).max(MIN_GUTTER_WIDTH)
}

pub fn header_left_padding(max_line_no: usize) -> usize {
    gutter_width_for_max_line(max_line_no).saturating_sub(1).saturating_sub(digit_count(max_line_no))
}

fn max_line_no_for_entries(pairs: impl IntoIterator<Item = (Option<usize>, Option<usize>)>) -> usize {
    pairs.into_iter().flat_map(|(left, right)| left.into_iter().chain(right)).max().unwrap_or(0)
}

pub fn usize_to_u16_saturating(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

/// Renders a diff preview, choosing split or unified based on terminal width
/// and whether the diff has removals.
///
/// For new files (only additions, no removals), uses unified view since
/// split view would have an empty left panel.
pub fn render_diff(preview: &DiffPreview, context: &ViewContext) -> Vec<Line> {
    let has_removals =
        preview.rows.iter().any(|row| row.left.as_ref().is_some_and(|entry| entry.tag == DiffTag::Removed));

    if context.size.width >= MIN_SPLIT_WIDTH && has_removals {
        highlight_split_diff(preview, context)
    } else {
        highlight_diff(preview, context)
    }
}

fn max_line_no_for_preview(preview: &DiffPreview) -> usize {
    max_line_no_for_entries(preview.rows.iter().map(|row| {
        (row.left.as_ref().and_then(|entry| entry.line_number), row.right.as_ref().and_then(|entry| entry.line_number))
    }))
}

fn highlight_split_diff(preview: &DiffPreview, context: &ViewContext) -> Vec<Line> {
    let theme = &context.theme;
    let dimensions = SplitLayoutDimensions::for_preview(preview, context.size.width as usize);

    let mut row_frames: Vec<Frame> = Vec::new();
    let mut visual_lines = 0usize;
    let mut rows_consumed = 0usize;

    for row in &preview.rows {
        let left_frame = render_entry(
            row.left.as_ref(),
            dimensions.left_content_width,
            &preview.lang_hint,
            Side::Left,
            dimensions.gutter_width,
            GutterTint::Neutral,
            context,
        );
        let right_frame = render_entry(
            row.right.as_ref(),
            dimensions.right_content_width,
            &preview.lang_hint,
            Side::Right,
            dimensions.gutter_width,
            GutterTint::Neutral,
            context,
        );

        let height = left_frame.lines().len().max(right_frame.lines().len());

        if visual_lines + height > MAX_DIFF_LINES && visual_lines > 0 {
            break;
        }

        let sep_line = Line::new(SEPARATOR.to_string());
        let sep_frame = Frame::new(vec![sep_line; height]);
        row_frames.push(Frame::hstack([
            FramePart::new(left_frame, dimensions.left_panel_width),
            FramePart::new(sep_frame, SEPARATOR_WIDTH_U16),
            FramePart::new(right_frame, dimensions.right_panel_width),
        ]));

        visual_lines += height;
        rows_consumed += 1;
    }

    let mut lines = Frame::vstack(row_frames).into_lines();

    if rows_consumed < preview.rows.len() {
        let remaining = preview.rows.len() - rows_consumed;
        let mut overflow = Line::default();
        overflow.push_styled(format!("    ... {remaining} more lines"), theme.muted());
        lines.push(overflow);
    }

    lines
}

fn blank_panel(width: usize) -> Line {
    Line::new(" ".repeat(width))
}

/// [`GutterTint::Diff`] extends the diff background and foreground through the
/// line-number gutter of changed rows, forming a clean color block. Inline
/// previews use [`GutterTint::Neutral`] so indenting the frame cannot pick up a
/// diff background (see [`Line::prepend`]).
pub fn render_entry(
    entry: Option<&SplitDiffEntry>,
    content_width: usize,
    lang_hint: &str,
    side: Side,
    gutter_width: usize,
    gutter_tint: GutterTint,
    context: &ViewContext,
) -> Frame {
    let theme = &context.theme;
    let inner_pad = if side == Side::Left { LEFT_INNER_PAD } else { 0 };
    let panel_width = gutter_width + content_width + inner_pad;

    let Some(entry) = entry else {
        return Frame::new(vec![blank_panel(panel_width)]);
    };

    let palette = DiffEntryStyle::for_entry(entry.tag, gutter_tint, theme);
    let highlighted = context.highlighter().highlight(&entry.content, lang_hint, theme);

    let content_line = if let Some(hl_line) = highlighted.first() {
        let mut styled_content = Line::default();
        for span in hl_line.spans() {
            styled_content.push_span(Span::with_style(span.text(), palette.content_style(span.style())));
        }
        styled_content
    } else {
        Line::with_style(&entry.content, palette.fallback_content_style())
    };
    let content_line = match palette.bg {
        Some(bg) => content_line.with_fill(bg),
        None => content_line,
    };

    let content_width_u16 = usize_to_u16_saturating(content_width);
    let extend_to = content_width + inner_pad;

    let digit_field = gutter_width.saturating_sub(1);
    let head = match entry.line_number {
        Some(num) => Line::with_style(format!("{num:>digit_field$} "), palette.gutter_style),
        None => Line::with_style(" ".repeat(gutter_width), palette.gutter_style),
    };
    let tail = Line::with_style(format!("{WRAP_MARKER:>digit_field$} "), palette.gutter_style.dim());

    Frame::new(vec![content_line])
        .fit(content_width_u16, FitOptions::wrap())
        .map_lines(move |mut line| {
            line.extend_bg_to_width(extend_to);
            line
        })
        .prefix(&head, &tail)
}

struct DiffEntryStyle {
    fg: Color,
    bg: Option<Color>,
    dim: bool,
    gutter_style: Style,
}

impl DiffEntryStyle {
    fn for_entry(tag: DiffTag, gutter_tint: GutterTint, theme: &crate::Theme) -> Self {
        let (fg, bg) = match tag {
            DiffTag::Removed => (theme.diff_removed_fg(), Some(theme.diff_removed_bg())),
            DiffTag::Added => (theme.diff_added_fg(), Some(theme.diff_added_bg())),
            DiffTag::Context => (theme.code_fg(), None),
        };
        let gutter_style = match (tag, gutter_tint) {
            (DiffTag::Removed, GutterTint::Diff) => {
                Style::fg(theme.diff_removed_fg()).bg_color(theme.diff_removed_bg())
            }
            (DiffTag::Added, GutterTint::Diff) => Style::fg(theme.diff_added_fg()).bg_color(theme.diff_added_bg()),
            _ => Style::fg(theme.muted()),
        };
        Self { fg, bg, dim: tag == DiffTag::Context, gutter_style }
    }

    fn content_style(&self, mut style: Style) -> Style {
        if let Some(bg) = self.bg {
            style.bg = Some(bg);
        }
        if self.dim {
            style.dim = true;
        }
        style
    }

    fn fallback_content_style(&self) -> Style {
        self.content_style(Style::fg(self.fg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::line::Line;
    use crate::{DiffLine, SplitDiffEntry, SplitDiffRow};

    fn test_context_with_width(width: u16) -> ViewContext {
        ViewContext::new((width, 24))
    }

    fn make_split_preview(rows: Vec<SplitDiffRow>) -> DiffPreview {
        DiffPreview { lines: vec![], rows, lang_hint: String::new(), start_line: None }
    }

    fn gutter_width_for_preview(preview: &DiffPreview) -> usize {
        gutter_width_for_max_line(max_line_no_for_preview(preview))
    }

    fn removed_entry(content: &str, line_num: usize) -> SplitDiffEntry {
        SplitDiffEntry::new(DiffTag::Removed, content, Some(line_num))
    }

    fn added_entry(content: &str, line_num: usize) -> SplitDiffEntry {
        SplitDiffEntry::new(DiffTag::Added, content, Some(line_num))
    }

    fn context_entry(content: &str, line_num: usize) -> SplitDiffEntry {
        SplitDiffEntry::new(DiffTag::Context, content, Some(line_num))
    }

    fn style_at_column(line: &Line, col: usize) -> Style {
        crate::testing::style_at_column(line, col)
    }

    #[test]
    fn wrapped_split_rows_preserve_neutral_boundary_columns() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry("LEFT_MARK", 1)),
            right: Some(added_entry(&format!("RIGHT_HEAD {} RIGHT_TAIL", "y".repeat(140)), 1)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);

        let first_row = lines
            .iter()
            .position(|line| {
                let text = line.plain_text();
                text.contains("LEFT_MARK") && text.contains("RIGHT_HEAD")
            })
            .expect("expected split row containing both left and right markers");

        let right_start =
            lines[first_row].plain_text().find("RIGHT_HEAD").expect("expected RIGHT_HEAD marker in first split row");

        let wrapped_row = lines
            .iter()
            .enumerate()
            .skip(first_row + 1)
            .find_map(|(index, line)| line.plain_text().contains("RIGHT_TAIL").then_some(index))
            .expect("expected wrapped continuation row containing RIGHT_TAIL marker");

        let wrapped_start = lines[wrapped_row]
            .plain_text()
            .find("RIGHT_TAIL")
            .expect("expected RIGHT_TAIL marker in wrapped continuation row");

        assert!(
            wrapped_start >= right_start,
            "wrapped continuation should not start left of original right-pane content start (was {wrapped_start}, expected >= {right_start})"
        );

        let added_bg = ctx.theme.diff_added_bg();
        let removed_bg = ctx.theme.diff_removed_bg();
        let gutter_width = gutter_width_for_preview(&preview);
        let padding_width = gutter_width + SEPARATOR_WIDTH;
        assert!(right_start >= padding_width, "right pane content should leave room for separator and gutter");
        for col in (right_start - padding_width)..right_start {
            let style = style_at_column(&lines[wrapped_row], col);
            assert_ne!(style.bg, Some(added_bg), "padding column {col} should not inherit added background");
            assert_ne!(style.bg, Some(removed_bg), "padding column {col} should not inherit removed background");
        }
    }

    #[test]
    fn both_panels_rendered_with_content() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry("old code", 1)),
            right: Some(added_entry("new code", 1)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert_eq!(lines.len(), 1);
        let text = lines[0].plain_text();
        assert!(text.contains("old code"), "left panel missing: {text}");
        assert!(text.contains("new code"), "right panel missing: {text}");
    }

    #[test]
    fn long_lines_wrapped_within_terminal_width() {
        let long = "x".repeat(200);
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry(&long, 1)),
            right: Some(added_entry(&long, 1)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert!(lines.len() > 1, "long line should wrap into multiple visual lines, got {}", lines.len());
        for line in &lines {
            let width = line.display_width();
            assert!(width <= 100, "line width {width} should not exceed terminal width 100");
        }
        // Full content should be present across all wrapped lines
        let all_text: String = lines.iter().map(Line::plain_text).collect();
        let x_count = all_text.chars().filter(|&c| c == 'x').count();
        // Both left and right panels contain 200 x's each
        assert_eq!(x_count, 400, "all content should be present across wrapped lines");
    }

    #[test]
    fn truncation_budget_applied() {
        let rows: Vec<SplitDiffRow> = (0..30)
            .map(|i| SplitDiffRow {
                left: Some(removed_entry(&format!("old {i}"), i + 1)),
                right: Some(added_entry(&format!("new {i}"), i + 1)),
            })
            .collect();
        let preview = make_split_preview(rows);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        // Short lines don't wrap, so 20 visual lines + 1 overflow
        assert_eq!(lines.len(), MAX_DIFF_LINES + 1);
        let last = lines.last().unwrap().plain_text();
        assert!(last.contains("more lines"), "overflow text missing: {last}");
    }

    #[test]
    fn empty_added_entry_pads_content_with_added_background() {
        let ctx = test_context_with_width(100);
        let added_bg = ctx.theme.diff_added_bg();
        let entry = SplitDiffEntry::new(DiffTag::Added, String::new(), Some(1));
        let content_width = 40;
        let gutter_width = MIN_GUTTER_WIDTH;
        let frame =
            render_entry(Some(&entry), content_width, "rs", Side::Right, gutter_width, GutterTint::Neutral, &ctx);
        let lines = frame.into_lines();
        assert_eq!(lines.len(), 1);
        let row = &lines[0];
        assert_eq!(
            row.display_width(),
            gutter_width + content_width,
            "row should fill panel width, got {}",
            row.display_width()
        );
        let trailing = row.spans().last().expect("row should have spans");
        assert_eq!(
            trailing.style().bg,
            Some(added_bg),
            "trailing pad of empty added entry should carry diff_added_bg, got {:?}",
            trailing.style().bg
        );
    }

    #[test]
    fn empty_preview_produces_no_output() {
        let preview = make_split_preview(vec![]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert!(lines.is_empty());
    }

    #[test]
    fn render_diff_dispatches_to_unified_below_80() {
        let preview = DiffPreview {
            lines: vec![DiffLine { tag: DiffTag::Removed, content: "old".to_string() }],
            rows: vec![SplitDiffRow { left: Some(removed_entry("old", 1)), right: None }],
            lang_hint: String::new(),
            start_line: None,
        };
        let ctx = test_context_with_width(79);
        let lines = render_diff(&preview, &ctx);
        // Unified renderer uses prefix "  - "
        assert!(
            lines[0].plain_text().contains("- old"),
            "should use unified renderer below 80: {}",
            lines[0].plain_text()
        );
    }

    #[test]
    fn new_file_uses_unified_view_even_at_wide_width() {
        // A new file (only additions, no removals) should use unified view
        // since split view would have an empty left panel
        let preview = DiffPreview {
            lines: vec![
                DiffLine { tag: DiffTag::Added, content: "fn main() {".to_string() },
                DiffLine { tag: DiffTag::Added, content: "    println!(\"Hello\");".to_string() },
                DiffLine { tag: DiffTag::Added, content: "}".to_string() },
            ],
            rows: vec![
                SplitDiffRow { left: None, right: Some(added_entry("fn main() {", 1)) },
                SplitDiffRow { left: None, right: Some(added_entry("    println!(\"Hello\");", 2)) },
                SplitDiffRow { left: None, right: Some(added_entry("}", 3)) },
            ],
            lang_hint: "rs".to_string(),
            start_line: None,
        };
        let ctx = test_context_with_width(100);
        let lines = render_diff(&preview, &ctx);
        // Unified renderer uses prefix "  + "
        let text = lines[0].plain_text();
        assert!(text.contains("+ fn main()"), "should use unified renderer for new file: {text}");
    }

    #[test]
    fn render_diff_dispatches_to_split_at_80() {
        let preview = DiffPreview {
            lines: vec![DiffLine { tag: DiffTag::Removed, content: "old".to_string() }],
            rows: vec![SplitDiffRow { left: Some(removed_entry("old", 1)), right: None }],
            lang_hint: String::new(),
            start_line: None,
        };
        let ctx = test_context_with_width(80);
        let lines = render_diff(&preview, &ctx);
        let text = lines[0].plain_text();
        // Split renderer shows line number gutter, not unified "- " prefix
        assert!(!text.contains("- old"), "should use split renderer at 80: {text}");
    }

    #[test]
    fn line_numbers_rendered_when_start_line_set() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(context_entry("hello", 42)),
            right: Some(context_entry("hello", 42)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        let text = lines[0].plain_text();
        assert!(text.contains("42"), "line number should be shown: {text}");
    }

    #[test]
    fn wrapped_row_pads_shorter_side_to_match_height() {
        // Left side has a long line that wraps, right side is short
        let long = "a".repeat(200);
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry(&long, 1)),
            right: Some(added_entry("short", 1)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert!(lines.len() > 1, "long left side should produce multiple visual lines");
        // All lines should have consistent width
        let first_width = lines[0].display_width();
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line.display_width(), first_width, "line {i} width mismatch");
        }
    }

    #[test]
    fn separator_has_no_background_on_context_row() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(context_entry("hello", 1)),
            right: Some(context_entry("world", 1)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert_eq!(lines.len(), 1);

        let gutter_width = gutter_width_for_preview(&preview);
        let usable = 100usize - (gutter_width * 2 + SEPARATOR_WIDTH + LEFT_INNER_PAD);
        let left_content = usable / 2;
        let sep_start = gutter_width + left_content + LEFT_INNER_PAD;
        let sep_end = sep_start + SEPARATOR_WIDTH;
        for col in sep_start..sep_end {
            let style = style_at_column(&lines[0], col);
            assert!(
                style.bg.is_none(),
                "separator column {col} should have no background on context row, got {:?}",
                style.bg
            );
        }
    }

    #[test]
    fn blank_gutter_when_line_number_none() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(SplitDiffEntry::new(DiffTag::Removed, "old", None)),
            right: None,
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        let text = lines[0].plain_text();
        let blank_gutter = " ".repeat(gutter_width_for_preview(&preview));
        assert!(text.starts_with(&blank_gutter), "should have blank gutter: {text:?}");
    }

    #[test]
    fn left_pane_diff_bg_extends_through_inner_pad_to_separator() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry("old", 1)),
            right: Some(added_entry("new", 1)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert_eq!(lines.len(), 1);

        let gutter_width = gutter_width_for_preview(&preview);
        let usable = 100usize - (gutter_width * 2 + SEPARATOR_WIDTH + LEFT_INNER_PAD);
        let left_content = usable / 2;
        let inner_pad_col = gutter_width + left_content + LEFT_INNER_PAD - 1;
        let style = style_at_column(&lines[0], inner_pad_col);
        assert_eq!(
            style.bg,
            Some(ctx.theme.diff_removed_bg()),
            "left inner-edge pad column {inner_pad_col} should carry diff_removed_bg, got {:?}",
            style.bg,
        );
    }

    #[test]
    fn absent_entry_renders_without_background() {
        let ctx = test_context_with_width(100);
        let frame = render_entry(None, 40, "", Side::Left, MIN_GUTTER_WIDTH, GutterTint::Diff, &ctx);
        let line = &frame.into_lines()[0];
        assert_eq!(style_at_column(line, 0).bg, None, "absent side should blend with the pane background");
    }

    #[test]
    fn tinted_gutter_carries_diff_colors() {
        let ctx = test_context_with_width(100);
        let frame =
            render_entry(Some(&removed_entry("old", 7)), 40, "", Side::Left, MIN_GUTTER_WIDTH, GutterTint::Diff, &ctx);
        let line = &frame.into_lines()[0];
        let style = style_at_column(line, 0);
        assert_eq!(style.bg, Some(ctx.theme.diff_removed_bg()), "tinted gutter should carry removed bg");
        assert_eq!(style.fg, Some(ctx.theme.diff_removed_fg()), "tinted gutter number should use removed fg");
    }

    #[test]
    fn inline_preview_gutter_stays_neutral() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry("old", 7)),
            right: Some(added_entry("new", 7)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        let style = style_at_column(&lines[0], 0);
        assert_eq!(style.bg, None, "inline preview gutter must stay neutral so frame indents do not inherit it");
    }

    #[test]
    fn wrapped_continuation_rows_show_wrap_marker() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry("short", 1)),
            right: Some(added_entry(&"y".repeat(140), 1)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert!(lines.len() > 1, "long right side should wrap");
        assert!(
            lines[1].plain_text().contains(WRAP_MARKER),
            "continuation row should mark the wrap in its gutter: {:?}",
            lines[1].plain_text()
        );
    }

    #[test]
    fn right_pane_line_number_sits_adjacent_to_separator() {
        let preview = make_split_preview(vec![SplitDiffRow {
            left: Some(removed_entry("old", 89)),
            right: Some(added_entry("new", 89)),
        }]);
        let ctx = test_context_with_width(100);
        let lines = highlight_split_diff(&preview, &ctx);
        assert_eq!(lines.len(), 1);

        let gutter_width = gutter_width_for_preview(&preview);
        let usable = 100usize - (gutter_width * 2 + SEPARATOR_WIDTH + LEFT_INNER_PAD);
        let left_content = usable / 2;
        let right_pane_first_col = gutter_width + left_content + LEFT_INNER_PAD + SEPARATOR_WIDTH;
        let text = lines[0].plain_text();
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(
            chars[right_pane_first_col], '8',
            "right pane line# digit '8' should sit at the very first column after SEP (col {right_pane_first_col}); got line: {text:?}",
        );
        assert_eq!(
            chars[right_pane_first_col + 1],
            '9',
            "right pane line# digit '9' should follow at col {}",
            right_pane_first_col + 1
        );
    }
}
