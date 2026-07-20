use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::wrap::wrap_line;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use similar::{DiffOp, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberedLine {
    pub number: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub old: Option<NumberedLine>,
    pub new: Option<NumberedLine>,
    pub kind: DiffKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPreview {
    pub language: String,
    pub rows: Vec<DiffRow>,
}

impl DiffPreview {
    pub fn compute(old: &str, new: &str, language: impl Into<String>) -> Self {
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();
        let mut rows = Vec::new();
        let mut old_number = 0;
        let mut new_number = 0;

        for operation in TextDiff::from_lines(old, new).ops() {
            match *operation {
                DiffOp::Equal { old_index, new_index, len } => {
                    for offset in 0..len {
                        old_number += 1;
                        new_number += 1;
                        rows.push(DiffRow {
                            old: Some(NumberedLine::new(old_number, line_at(&old_lines, old_index + offset))),
                            new: Some(NumberedLine::new(new_number, line_at(&new_lines, new_index + offset))),
                            kind: DiffKind::Context,
                        });
                    }
                }
                DiffOp::Delete { old_index, old_len, .. } => {
                    for offset in 0..old_len {
                        old_number += 1;
                        rows.push(DiffRow {
                            old: Some(NumberedLine::new(old_number, line_at(&old_lines, old_index + offset))),
                            new: None,
                            kind: DiffKind::Removed,
                        });
                    }
                }
                DiffOp::Insert { new_index, new_len, .. } => {
                    for offset in 0..new_len {
                        new_number += 1;
                        rows.push(DiffRow {
                            old: None,
                            new: Some(NumberedLine::new(new_number, line_at(&new_lines, new_index + offset))),
                            kind: DiffKind::Added,
                        });
                    }
                }
                DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                    for offset in 0..old_len.max(new_len) {
                        let old_line = (offset < old_len).then(|| {
                            old_number += 1;
                            NumberedLine::new(old_number, line_at(&old_lines, old_index + offset))
                        });
                        let new_line = (offset < new_len).then(|| {
                            new_number += 1;
                            NumberedLine::new(new_number, line_at(&new_lines, new_index + offset))
                        });
                        rows.push(DiffRow { old: old_line, new: new_line, kind: DiffKind::Changed });
                    }
                }
            }
        }

        trim_context(&mut rows);
        Self { language: language.into(), rows }
    }
}

impl NumberedLine {
    fn new(number: usize, text: &str) -> Self {
        Self { number, text: text.to_string() }
    }
}

pub fn render_diff(
    preview: &DiffPreview,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let has_removals = preview.rows.iter().any(|row| row.old.is_some() && row.kind != DiffKind::Context);
    if width >= 80 && has_removals {
        render_split_diff(preview, width, theme, highlighter)
    } else {
        render_unified_diff(preview, width, theme, highlighter)
    }
}

fn render_unified_diff(
    preview: &DiffPreview,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    const MAX_LINES: usize = 20;
    let mut lines = Vec::new();
    for row in &preview.rows {
        match row.kind {
            DiffKind::Context => {
                if let Some(line) = &row.old {
                    lines.extend(render_diff_line(line, " ", None, preview, width, theme, highlighter));
                }
            }
            DiffKind::Removed => {
                if let Some(line) = &row.old {
                    lines.extend(render_diff_line(
                        line,
                        "-",
                        Some((theme.diff_removed_fg, theme.diff_removed_bg)),
                        preview,
                        width,
                        theme,
                        highlighter,
                    ));
                }
            }
            DiffKind::Added => {
                if let Some(line) = &row.new {
                    lines.extend(render_diff_line(
                        line,
                        "+",
                        Some((theme.diff_added_fg, theme.diff_added_bg)),
                        preview,
                        width,
                        theme,
                        highlighter,
                    ));
                }
            }
            DiffKind::Changed => {
                if let Some(line) = &row.old {
                    lines.extend(render_diff_line(
                        line,
                        "-",
                        Some((theme.diff_removed_fg, theme.diff_removed_bg)),
                        preview,
                        width,
                        theme,
                        highlighter,
                    ));
                }
                if let Some(line) = &row.new {
                    lines.extend(render_diff_line(
                        line,
                        "+",
                        Some((theme.diff_added_fg, theme.diff_added_bg)),
                        preview,
                        width,
                        theme,
                        highlighter,
                    ));
                }
            }
        }
    }

    if lines.len() > MAX_LINES {
        let remaining = lines.len() - MAX_LINES;
        lines.truncate(MAX_LINES);
        lines.push(Line::styled(format!("    … {remaining} more lines"), Style::new().fg(theme.muted)));
    }
    lines
}

fn render_split_diff(
    preview: &DiffPreview,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    const MAX_ROWS: usize = 20;
    let left_width = width.saturating_sub(1) / 2;
    let right_width = width.saturating_sub(1 + left_width);
    let mut lines = Vec::new();

    for row in preview.rows.iter().take(MAX_ROWS) {
        let left_colors = matches!(row.kind, DiffKind::Removed | DiffKind::Changed)
            .then_some((theme.diff_removed_fg, theme.diff_removed_bg));
        let right_colors = matches!(row.kind, DiffKind::Added | DiffKind::Changed)
            .then_some((theme.diff_added_fg, theme.diff_added_bg));
        let left = render_split_panel(row.old.as_ref(), left_width, left_colors, preview, theme, highlighter);
        let right = render_split_panel(row.new.as_ref(), right_width, right_colors, preview, theme, highlighter);
        let mut spans = left.spans;
        spans.push(Span::styled(" ", Style::new().bg(theme.background)));
        spans.extend(right.spans);
        lines.push(Line::from(spans));
    }

    if preview.rows.len() > MAX_ROWS {
        lines.push(Line::styled(
            format!("    … {} more rows", preview.rows.len() - MAX_ROWS),
            Style::new().fg(theme.muted),
        ));
    }
    lines
}

fn render_split_panel(
    numbered: Option<&NumberedLine>,
    width: u16,
    colors: Option<(ratatui::style::Color, ratatui::style::Color)>,
    preview: &DiffPreview,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let (foreground, background) = colors.unwrap_or((theme.text_secondary, theme.background));
    let mut line = if let Some(numbered) = numbered {
        let mut spans =
            vec![Span::styled(format!("{:>4} ", numbered.number), Style::new().fg(foreground).bg(background))];
        if let Some(highlighted) = highlighter.highlight(&numbered.text, &preview.language, theme).first() {
            spans.extend(highlighted.spans.iter().cloned().map(|mut span| {
                span.style = span.style.patch(Style::new().bg(background));
                span
            }));
        }
        let source = Line::from(spans).style(Style::new().bg(background));
        let wrapped = wrap_line(source.clone(), width);
        if wrapped.len() > 1 {
            truncate_line(source, width, Style::new().fg(foreground).bg(background))
        } else {
            wrapped.into_iter().next().unwrap_or_default()
        }
    } else {
        Line::default()
    };

    let current_width = line.width();
    if current_width < usize::from(width) {
        line.spans.push(Span::styled(" ".repeat(usize::from(width) - current_width), Style::new().bg(background)));
    }
    line
}

fn truncate_line(line: Line<'static>, width: u16, indicator_style: Style) -> Line<'static> {
    if width <= 1 {
        return Line::styled("…", indicator_style);
    }

    let content_width = width - 1;
    let mut line = wrap_line(line, content_width).into_iter().next().unwrap_or_default();
    let padding = usize::from(content_width).saturating_sub(line.width());
    if padding > 0 {
        line.spans.push(Span::styled(" ".repeat(padding), indicator_style));
    }
    line.spans.push(Span::styled("…", indicator_style));
    line
}

fn render_diff_line(
    numbered: &NumberedLine,
    marker: &str,
    colors: Option<(ratatui::style::Color, ratatui::style::Color)>,
    preview: &DiffPreview,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let (foreground, background) = colors.unwrap_or((theme.text_secondary, theme.background));
    let mut spans =
        vec![Span::styled(format!("{:>4} {marker} ", numbered.number), Style::new().fg(foreground).bg(background))];
    let syntax_lines = highlighter.highlight(&numbered.text, &preview.language, theme);
    if let Some(line) = syntax_lines.first() {
        spans.extend(line.spans.iter().cloned().map(|mut span| {
            span.style = span.style.patch(Style::new().bg(background));
            span
        }));
    }
    wrap_line(Line::from(spans).style(Style::new().bg(background)), width)
}

fn line_at<'a>(lines: &[&'a str], index: usize) -> &'a str {
    lines.get(index).copied().unwrap_or("")
}

fn trim_context(rows: &mut Vec<DiffRow>) {
    const CONTEXT: usize = 3;
    let Some(first_change) = rows.iter().position(|row| row.kind != DiffKind::Context) else {
        return;
    };
    let last_change = rows.iter().rposition(|row| row.kind != DiffKind::Context).unwrap_or(first_change);
    let start = first_change.saturating_sub(CONTEXT);
    let end = (last_change + CONTEXT + 1).min(rows.len());
    rows.drain(end..);
    rows.drain(..start);
}
