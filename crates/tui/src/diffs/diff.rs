use crossterm::style::Color;

use crate::{DiffPreview, DiffTag};

use crate::line::Line;
use crate::rendering::render_context::ViewContext;
use crate::span::Span;
use crate::style::Style;
use crate::theme::Theme;

const MAX_DIFF_LINES: usize = 20;

struct DiffStyle<'a> {
    prefix: &'a str,
    fg: Color,
    bg: Option<Color>,
}

/// Render a diff preview with syntax-highlighted context/removed/added lines.
///
/// Context lines are shown with a `"    "` prefix and code background.
/// Removed lines are shown with a `"  - "` prefix and red-tinted background.
/// Added lines are shown with a `"  + "` prefix and green-tinted background.
pub fn highlight_diff(preview: &DiffPreview, context: &ViewContext) -> Vec<Line> {
    let theme: &Theme = &context.theme;
    let total = preview.lines.len();
    let truncated = total > MAX_DIFF_LINES;
    let budget = if truncated { MAX_DIFF_LINES } else { total };

    let mut lines = Vec::with_capacity(budget + usize::from(truncated));

    let context_style = DiffStyle { prefix: "    ", fg: theme.code_fg(), bg: None };
    let removed_style = DiffStyle { prefix: "  - ", fg: theme.diff_removed_fg(), bg: Some(theme.diff_removed_bg()) };
    let added_style = DiffStyle { prefix: "  + ", fg: theme.diff_added_fg(), bg: Some(theme.diff_added_bg()) };

    let mut old_line = preview.start_line.unwrap_or(0);

    for diff_line in preview.lines.iter().take(budget) {
        let style = match diff_line.tag {
            DiffTag::Context => &context_style,
            DiffTag::Removed => &removed_style,
            DiffTag::Added => &added_style,
        };

        let mut line = Line::default();

        if preview.start_line.is_some() {
            match diff_line.tag {
                DiffTag::Context | DiffTag::Removed => {
                    let line_num = format!("{old_line:>4} ");
                    line.push_styled(line_num, theme.muted());
                }
                DiffTag::Added => {
                    line.push_styled("     ", theme.muted());
                }
            }
        }

        let mut prefix_style = Style::fg(style.fg);
        if let Some(bg) = style.bg {
            prefix_style = prefix_style.bg_color(bg);
        }
        line.push_span(Span::with_style(style.prefix, prefix_style));

        let spans = context.highlighter().highlight(&diff_line.content, &preview.lang_hint, theme);
        if let Some(content) = spans.first() {
            for span in content.spans() {
                let mut span_style = span.style();
                if let Some(bg) = style.bg {
                    span_style.bg = Some(bg);
                }
                line.push_span(Span::with_style(span.text(), span_style));
            }
        }
        lines.push(line);

        if matches!(diff_line.tag, DiffTag::Context | DiffTag::Removed) {
            old_line += 1;
        }
    }

    if truncated {
        let remaining = total - budget;
        let mut overflow = Line::default();
        overflow.push_styled(format!("    ... {remaining} more lines"), theme.muted());
        lines.push(overflow);
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffLine;

    fn test_context() -> ViewContext {
        ViewContext::new((80, 24))
    }

    fn test_theme() -> Theme {
        Theme::default()
    }

    fn make_preview(lines: Vec<DiffLine>) -> DiffPreview {
        DiffPreview { lines, rows: vec![], lang_hint: String::new(), start_line: None }
    }

    #[test]
    fn removed_lines_have_minus_prefix() {
        let preview = make_preview(vec![DiffLine { tag: DiffTag::Removed, content: "old line".to_string() }]);
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().contains("- old line"));
    }

    #[test]
    fn added_lines_have_plus_prefix() {
        let preview = make_preview(vec![DiffLine { tag: DiffTag::Added, content: "new line".to_string() }]);
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().contains("+ new line"));
    }

    #[test]
    fn context_lines_have_no_diff_prefix() {
        let preview = make_preview(vec![DiffLine { tag: DiffTag::Context, content: "unchanged".to_string() }]);
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 1);
        let text = lines[0].plain_text();
        assert!(text.starts_with("    "), "context should have space prefix: {text}");
        assert!(!text.contains("+ "), "context should not have + prefix");
        assert!(!text.contains("- "), "context should not have - prefix");
    }

    #[test]
    fn mixed_diff_renders_correctly() {
        let preview = make_preview(vec![
            DiffLine { tag: DiffTag::Context, content: "before".to_string() },
            DiffLine { tag: DiffTag::Removed, content: "old".to_string() },
            DiffLine { tag: DiffTag::Added, content: "new".to_string() },
            DiffLine { tag: DiffTag::Context, content: "after".to_string() },
        ]);
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 4);
        assert!(lines[0].plain_text().contains("before"));
        assert!(lines[1].plain_text().contains("- old"));
        assert!(lines[2].plain_text().contains("+ new"));
        assert!(lines[3].plain_text().contains("after"));
    }

    #[test]
    fn both_removed_and_added() {
        let preview = make_preview(vec![
            DiffLine { tag: DiffTag::Removed, content: "old".to_string() },
            DiffLine { tag: DiffTag::Added, content: "new".to_string() },
        ]);
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 2);
        assert!(lines[0].plain_text().contains("- old"));
        assert!(lines[1].plain_text().contains("+ new"));
    }

    #[test]
    fn truncates_long_diffs() {
        let diff_lines: Vec<DiffLine> = (0..30)
            .map(|i| DiffLine {
                tag: if i % 2 == 0 { DiffTag::Removed } else { DiffTag::Added },
                content: format!("line {i}"),
            })
            .collect();
        let preview = make_preview(diff_lines);
        let lines = highlight_diff(&preview, &test_context());
        // 20 content lines + 1 overflow line
        assert_eq!(lines.len(), MAX_DIFF_LINES + 1);
        let last = lines.last().unwrap().plain_text();
        assert!(last.contains("more lines"), "Expected overflow text: {last}");
    }

    #[test]
    fn no_truncation_at_boundary() {
        let diff_lines: Vec<DiffLine> = (0..20)
            .map(|i| DiffLine {
                tag: if i % 2 == 0 { DiffTag::Removed } else { DiffTag::Added },
                content: format!("line {i}"),
            })
            .collect();
        let preview = make_preview(diff_lines);
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 20);
        assert!(!lines.last().unwrap().plain_text().contains("more lines"));
    }

    #[test]
    fn syntax_highlighting_with_known_lang() {
        let preview = DiffPreview {
            lines: vec![
                DiffLine { tag: DiffTag::Removed, content: "fn old() {}".to_string() },
                DiffLine { tag: DiffTag::Added, content: "fn new() {}".to_string() },
            ],
            rows: vec![],
            lang_hint: "rs".to_string(),
            start_line: None,
        };
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 2);
        // With syntax highlighting, there should be multiple spans (not just 2: prefix + text)
        assert!(lines[0].spans().len() > 2, "Expected syntax-highlighted spans, got {} spans", lines[0].spans().len());
    }

    #[test]
    fn removed_lines_have_red_bg() {
        let preview = make_preview(vec![DiffLine { tag: DiffTag::Removed, content: "old".to_string() }]);
        let theme = test_theme();
        let ctx = test_context();
        let lines = highlight_diff(&preview, &ctx);
        let prefix_span = &lines[0].spans()[0];
        assert_eq!(prefix_span.style().bg, Some(theme.diff_removed_bg()));
        assert_eq!(prefix_span.style().fg, Some(theme.diff_removed_fg()));
    }

    #[test]
    fn added_lines_have_green_bg() {
        let preview = make_preview(vec![DiffLine { tag: DiffTag::Added, content: "new".to_string() }]);
        let theme = test_theme();
        let ctx = test_context();
        let lines = highlight_diff(&preview, &ctx);
        let prefix_span = &lines[0].spans()[0];
        assert_eq!(prefix_span.style().bg, Some(theme.diff_added_bg()));
        assert_eq!(prefix_span.style().fg, Some(theme.diff_added_fg()));
    }

    #[test]
    fn context_lines_have_code_bg() {
        let preview = make_preview(vec![DiffLine { tag: DiffTag::Context, content: "same".to_string() }]);
        let ctx = test_context();
        let lines = highlight_diff(&preview, &ctx);
        let prefix_span = &lines[0].spans()[0];
        assert_eq!(prefix_span.style().bg, None);
    }

    #[test]
    fn empty_diff_produces_no_lines() {
        let preview = make_preview(vec![]);
        let lines = highlight_diff(&preview, &test_context());
        assert!(lines.is_empty());
    }

    #[test]
    fn line_numbers_rendered_when_start_line_set() {
        let preview = DiffPreview {
            lines: vec![
                DiffLine { tag: DiffTag::Context, content: "ctx".to_string() },
                DiffLine { tag: DiffTag::Removed, content: "old".to_string() },
                DiffLine { tag: DiffTag::Added, content: "new".to_string() },
                DiffLine { tag: DiffTag::Context, content: "ctx2".to_string() },
            ],
            rows: vec![],
            lang_hint: String::new(),
            start_line: Some(10),
        };
        let lines = highlight_diff(&preview, &test_context());
        assert_eq!(lines.len(), 4);
        assert!(lines[0].plain_text().contains("10"), "context line should show 10");
        assert!(lines[1].plain_text().contains("11"), "removed line should show 11");
        // Added lines don't show a source line number
        let added_text = lines[2].plain_text();
        assert!(!added_text.starts_with("  12"), "added line should not show line number");
        assert!(lines[3].plain_text().contains("12"), "next context should show 12");
    }

    #[test]
    fn focused_preview_with_truncation_shows_changes() {
        // Simulates a DiffPreview produced by compute_diff_preview after trimming:
        // 3 context lines, then changes, then 22 more context lines (total 27 > MAX_DIFF_LINES).
        let mut diff_lines: Vec<DiffLine> =
            (0..3).map(|_| DiffLine { tag: DiffTag::Context, content: "before".to_string() }).collect();

        diff_lines.push(DiffLine { tag: DiffTag::Removed, content: "old".to_string() });

        diff_lines.push(DiffLine { tag: DiffTag::Added, content: "new".to_string() });

        diff_lines.extend((0..22).map(|_| DiffLine { tag: DiffTag::Context, content: "after".to_string() }));

        let preview = DiffPreview { lines: diff_lines, rows: vec![], lang_hint: String::new(), start_line: Some(42) };

        let lines = highlight_diff(&preview, &test_context());
        let has_change = lines.iter().any(|l| {
            let text = l.plain_text();
            text.contains("- old") || text.contains("+ new")
        });

        assert!(has_change, "focused preview should show changes within the truncation budget");
    }

    #[test]
    fn no_line_numbers_when_start_line_none() {
        let preview = make_preview(vec![DiffLine { tag: DiffTag::Removed, content: "old".to_string() }]);
        let lines = highlight_diff(&preview, &test_context());
        let text = lines[0].plain_text();
        // Without line numbers, the line should start with the prefix directly
        assert!(text.starts_with("  - "), "expected prefix without line number gutter: {text}");
    }
}
