use super::PatchAnchor;
use crate::components::common::AnchoredSurfaceBuilder;
use crate::components::review_comments::{AnchoredRows, CommentAnchor};
use crate::git_diff::{FileDiff, PatchLineKind};
use tui::{Line, ViewContext};

pub(crate) struct RenderedPatch {
    pub surface: AnchoredRows<PatchAnchor>,
}

impl RenderedPatch {
    pub(crate) fn new(surface: AnchoredRows<PatchAnchor>) -> Self {
        Self { surface }
    }

    pub(crate) fn from_file_diff(file: &FileDiff, width: usize, ctx: &ViewContext) -> RenderedPatch {
        let theme = &ctx.theme;
        let lang_hint = lang_hint_from_path(&file.path);
        let max_line_no = file.max_line_no();

        let gutter_width = tui::digit_count(max_line_no);
        let width_u16 = tui::usize_to_u16_saturating(width);
        let mut rows = AnchoredSurfaceBuilder::new();

        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            for (line_idx, patch_line) in hunk.lines.iter().enumerate() {
                let (head, tail, content) = match patch_line.kind {
                    PatchLineKind::HunkHeader => continue,
                    PatchLineKind::Meta => (
                        Line::default(),
                        Line::default(),
                        Line::with_style(&patch_line.text, tui::Style::fg(theme.text_secondary()).italic()),
                    ),
                    PatchLineKind::Context => {
                        let line_str = format_line_no(patch_line.new_line_no.or(patch_line.old_line_no), gutter_width);
                        let head = Line::with_style(format!("{line_str}   "), tui::Style::fg(theme.text_secondary()));
                        let tail = wrap_marker_gutter(gutter_width, tui::Style::fg(theme.muted()).dim());
                        let mut content = Line::default();
                        append_syntax_spans(&mut content, &patch_line.text, lang_hint, None, ctx);
                        (head, tail, content)
                    }
                    PatchLineKind::Added => {
                        let line_str = format_line_no(patch_line.new_line_no, gutter_width);
                        let bg = theme.diff_added_bg();
                        let head = Line::with_style(
                            format!("{line_str} + "),
                            tui::Style::fg(theme.diff_added_fg()).bg_color(bg),
                        );
                        let tail =
                            wrap_marker_gutter(gutter_width, tui::Style::fg(theme.diff_added_fg()).dim().bg_color(bg));
                        let mut content = Line::default();
                        append_syntax_spans(&mut content, &patch_line.text, lang_hint, Some(bg), ctx);
                        let content = content.with_fill(bg);
                        (head, tail, content)
                    }
                    PatchLineKind::Removed => {
                        let line_str = format_line_no(patch_line.old_line_no, gutter_width);
                        let bg = theme.diff_removed_bg();
                        let head = Line::with_style(
                            format!("{line_str} - "),
                            tui::Style::fg(theme.diff_removed_fg()).bg_color(bg),
                        );
                        let tail = wrap_marker_gutter(
                            gutter_width,
                            tui::Style::fg(theme.diff_removed_fg()).dim().bg_color(bg),
                        );
                        let mut content = Line::default();
                        append_syntax_spans(&mut content, &patch_line.text, lang_hint, Some(bg), ctx);
                        let content = content.with_fill(bg);
                        (head, tail, content)
                    }
                };

                let anchor = CommentAnchor(PatchAnchor { hunk: hunk_idx, line: line_idx });
                rows.push_anchored_wrapped(anchor, content, width_u16, &head, &tail);
            }
        }

        Self::new(rows.finish())
    }
}

/// Gutter prefix for wrapped continuation rows: a dim `↪` right-aligned in the
/// line-number gutter, matching the head width used by unified diffs.
fn wrap_marker_gutter(gutter_width: usize, style: tui::Style) -> Line {
    Line::with_style(format!("{:>width$}  ", tui::WRAP_MARKER, width = gutter_width + 1), style)
}

fn append_syntax_spans(
    line: &mut Line,
    text: &str,
    lang_hint: &str,
    bg_override: Option<tui::Color>,
    ctx: &ViewContext,
) {
    let spans = ctx.highlighter().highlight(text, lang_hint, &ctx.theme);
    if let Some(content) = spans.first() {
        for span in content.spans() {
            let mut span_style = span.style();
            if let Some(bg) = bg_override {
                span_style.bg = Some(bg);
            }
            line.push_span(tui::Span::with_style(span.text(), span_style));
        }
    } else {
        line.push_text(text);
    }
}

fn format_line_no(line_no: Option<usize>, width: usize) -> String {
    match line_no {
        Some(line_no) => format!("{line_no:>width$}"),
        None => " ".repeat(width),
    }
}

pub(crate) fn lang_hint_from_path(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff::{FileDiff, FileStatus, Hunk, PatchLine, StageState};

    fn make_file(lines: Vec<PatchLine>) -> FileDiff {
        FileDiff {
            old_path: Some("test.rs".to_string()),
            path: "test.rs".to_string(),
            status: FileStatus::Modified,
            staged: StageState::Unstaged,
            hunks: vec![Hunk {
                header: "@@ -1,1 +1,1 @@".to_string(),
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines,
            }],
            binary: false,
        }
    }

    #[test]
    fn rendered_patch_contains_insert_row_lookup() {
        let file = make_file(vec![
            PatchLine {
                kind: PatchLineKind::HunkHeader,
                text: "@@ -1,1 +1,1 @@".to_string(),
                old_line_no: None,
                new_line_no: None,
            },
            PatchLine {
                kind: PatchLineKind::Context,
                text: "fn test()".to_string(),
                old_line_no: Some(1),
                new_line_no: Some(1),
            },
        ]);
        let context = ViewContext::new((120, 24));
        let result = RenderedPatch::from_file_diff(&file, 80, &context);

        assert_eq!(result.surface.groups().len(), 1);
        assert_eq!(result.surface.end_row_for_anchor(CommentAnchor(PatchAnchor { hunk: 0, line: 1 })), Some(0));
    }

    #[test]
    fn long_lines_soft_wrapped_to_right_width() {
        let long_content = "x".repeat(200);
        let file = make_file(vec![
            PatchLine {
                kind: PatchLineKind::HunkHeader,
                text: "@@ -1,1 +1,1 @@".to_string(),
                old_line_no: None,
                new_line_no: None,
            },
            PatchLine { kind: PatchLineKind::Added, text: long_content, old_line_no: None, new_line_no: Some(1) },
        ]);
        let context = ViewContext::new((120, 24));
        let right_width = 60;
        let result = RenderedPatch::from_file_diff(&file, right_width, &context);

        assert!(result.surface.lines().len() > 2, "long line should wrap, got {} lines", result.surface.lines().len());

        for (index, line) in result.surface.lines().iter().enumerate() {
            let display_width = line.display_width();
            assert!(
                display_width <= right_width,
                "line {index} width {display_width} exceeds right_width {right_width}"
            );
        }

        let anchor = CommentAnchor(PatchAnchor { hunk: 0, line: 1 });
        assert_eq!(result.surface.start_row_for_anchor(anchor), Some(0));
        assert!(
            result.surface.end_row_for_anchor(anchor).is_some_and(|end_row| end_row > 0),
            "wrapped line should extend the anchored block"
        );

        let gutter_pad = format!("{:>width$}  ", tui::WRAP_MARKER, width = tui::digit_count(1) + 1);
        for (index, line) in result.surface.lines().iter().enumerate().skip(1) {
            let text = line.plain_text();
            assert!(
                text.starts_with(&gutter_pad),
                "continuation line {index} should start with a wrap marker gutter, got {text:?}"
            );
        }
    }

    #[test]
    fn added_row_continuation_preserves_added_background() {
        let long_content = "x".repeat(200);
        let file = make_file(vec![
            PatchLine {
                kind: PatchLineKind::HunkHeader,
                text: "@@ -1,1 +1,1 @@".to_string(),
                old_line_no: None,
                new_line_no: None,
            },
            PatchLine { kind: PatchLineKind::Added, text: long_content, old_line_no: None, new_line_no: Some(1) },
        ]);
        let context = ViewContext::new((120, 24));
        let added_bg = context.theme.diff_added_bg();
        let result = RenderedPatch::from_file_diff(&file, 60, &context);

        assert!(result.surface.lines().len() > 2, "long Added line should wrap");

        for (index, line) in result.surface.lines().iter().enumerate().skip(1) {
            let first_span = line.spans().first().expect("continuation row should have spans");
            assert_eq!(
                first_span.style().bg,
                Some(added_bg),
                "continuation line {index}: gutter tail should carry diff_added_bg, got {:?}",
                first_span.style().bg
            );
        }
    }

    #[test]
    fn empty_added_line_fills_full_width_with_added_background() {
        let file = make_file(vec![
            PatchLine {
                kind: PatchLineKind::HunkHeader,
                text: "@@ -1,1 +1,1 @@".to_string(),
                old_line_no: None,
                new_line_no: None,
            },
            PatchLine { kind: PatchLineKind::Added, text: String::new(), old_line_no: None, new_line_no: Some(1) },
        ]);
        let context = ViewContext::new((120, 24));
        let added_bg = context.theme.diff_added_bg();
        let total_width = 60;
        let result = RenderedPatch::from_file_diff(&file, total_width, &context);

        let added_row = &result.surface.lines()[0];
        assert_eq!(
            added_row.display_width(),
            total_width,
            "empty added row should be padded to full width, got {}",
            added_row.display_width()
        );
        let trailing = added_row.spans().last().expect("added row should have at least one span");
        assert_eq!(
            trailing.style().bg,
            Some(added_bg),
            "trailing pad of empty added line should carry diff_added_bg, got {:?}",
            trailing.style().bg
        );
    }

    #[test]
    fn lang_hint_extracts_extension() {
        assert_eq!(lang_hint_from_path("src/main.rs"), "rs");
        assert_eq!(lang_hint_from_path("foo.py"), "py");
        assert_eq!(lang_hint_from_path("Makefile"), "Makefile");
        assert_eq!(lang_hint_from_path("a/b/c.tsx"), "tsx");
    }
}
