use crate::components::syntax::SyntaxHighlighter;
use crate::components::theme::Theme;
use crate::components::wrap::{fit_line, wrap_line};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use similar::{DiffOp, TextDiff};

pub const SPLIT_VIEW_MIN_WIDTH: u16 = 96;

/// The column a split view puts between its two halves.
const SPLIT_SEPARATOR: &str = "│";

/// How a diff row is tinted. Both the inline previews in the transcript and the
/// full git-diff screen render through this, so a line looks the same wherever
/// it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTone {
    Context,
    Added,
    Removed,
}

/// A diff line rendered but not yet fitted to the viewport: callers wrap it or
/// truncate it, using `fill` for the padding and ellipsis they add.
pub struct DiffLine {
    pub line: Line<'static>,
    pub fill: Style,
}

impl DiffTone {
    pub fn colors(self, theme: &Theme) -> (Color, Color) {
        match self {
            Self::Context => (theme.text_secondary, theme.background),
            Self::Added => (theme.diff_added_fg, theme.diff_added_bg),
            Self::Removed => (theme.diff_removed_fg, theme.diff_removed_bg),
        }
    }
}

/// Renders `gutter` (line numbers and any change marker) followed by the
/// syntax-highlighted source, tinted for `tone`.
pub fn diff_line(
    gutter: &str,
    text: &str,
    language: &str,
    tone: DiffTone,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> DiffLine {
    let (foreground, background) = tone.colors(theme);
    let fill = Style::new().fg(foreground).bg(background);
    let mut spans = vec![Span::styled(gutter.to_string(), fill)];
    spans.extend(highlighted_spans(text, language, background, theme, highlighter));
    DiffLine { line: Line::from(spans).style(Style::new().bg(background)), fill }
}

/// One half of a split diff, occupying exactly `width` columns. An absent line
/// renders as an empty, still-tinted gap so the two sides stay aligned.
pub fn split_side(
    number: Option<usize>,
    text: Option<&str>,
    language: &str,
    tone: DiffTone,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let gutter = number.map_or_else(|| "     ".to_string(), |number| format!("{number:>4} "));
    let rendered = diff_line(&gutter, text.unwrap_or_default(), language, tone, theme, highlighter);
    fit_line(rendered.line, usize::from(width), rendered.fill)
}

/// Column widths for the two halves of a split view `width` columns wide.
pub fn split_widths(width: u16) -> (u16, u16) {
    let left = width.saturating_sub(1) / 2;
    (left, width.saturating_sub(left + 1))
}

/// Joins two rendered halves with the separator column between them.
pub fn join_split(left: Line<'static>, right: Line<'static>, theme: &Theme) -> Line<'static> {
    let mut spans = left.spans;
    spans.push(Span::styled(SPLIT_SEPARATOR, Style::new().fg(theme.muted).bg(theme.background)));
    spans.extend(right.spans);
    Line::from(spans)
}

/// Syntax-highlighted spans for one source line, re-tinted to sit on
/// `background` so the highlighting does not punch holes in a diff row.
pub fn highlighted_spans(
    source: &str,
    language: &str,
    background: Color,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Span<'static>> {
    let lines = highlighter.highlight(source, language, theme);
    let Some(first) = lines.first() else {
        return vec![Span::styled(source.to_string(), Style::new().bg(background))];
    };
    first
        .spans
        .iter()
        .map(|span| {
            let mut span = span.clone();
            span.style = span.style.patch(Style::new().bg(background));
            span
        })
        .collect()
}

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

impl DiffRow {
    /// The sides a unified view draws for this row, with the tone and marker
    /// each is drawn in. A changed row draws both, removal first.
    fn unified_sides(&self) -> Vec<(Option<&NumberedLine>, DiffTone, char)> {
        match self.kind {
            DiffKind::Context => vec![(self.old.as_ref(), DiffTone::Context, ' ')],
            DiffKind::Removed => vec![(self.old.as_ref(), DiffTone::Removed, '-')],
            DiffKind::Added => vec![(self.new.as_ref(), DiffTone::Added, '+')],
            DiffKind::Changed => {
                vec![(self.old.as_ref(), DiffTone::Removed, '-'), (self.new.as_ref(), DiffTone::Added, '+')]
            }
        }
    }

    /// Tones for the left and right halves of a split view. A side with no line
    /// still carries the change tone, so the gap it leaves reads as part of the
    /// change rather than as context.
    fn split_tones(&self) -> (DiffTone, DiffTone) {
        let left = match self.kind {
            DiffKind::Removed | DiffKind::Changed => DiffTone::Removed,
            _ => DiffTone::Context,
        };
        let right = match self.kind {
            DiffKind::Added | DiffKind::Changed => DiffTone::Added,
            _ => DiffTone::Context,
        };
        (left, right)
    }
}

pub fn render_diff(
    preview: &DiffPreview,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let has_removals = preview.rows.iter().any(|row| row.old.is_some() && row.kind != DiffKind::Context);
    if width >= SPLIT_VIEW_MIN_WIDTH && has_removals {
        render_split_diff(preview, width, theme, highlighter)
    } else {
        render_unified_diff(preview, width, theme, highlighter)
    }
}

/// Most rows an inline preview shows before collapsing the rest into a count.
const MAX_PREVIEW_ROWS: usize = 20;

fn render_unified_diff(
    preview: &DiffPreview,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for row in preview.rows.iter().take(MAX_PREVIEW_ROWS) {
        for (side, tone, marker) in row.unified_sides() {
            let Some(side) = side else {
                continue;
            };
            let gutter = format!("{:>4} {marker} ", side.number);
            let rendered = diff_line(&gutter, &side.text, &preview.language, tone, theme, highlighter);
            lines.extend(wrap_line(rendered.line, width));
        }
    }
    lines.extend(truncation_notice(preview, theme));
    lines
}

fn render_split_diff(
    preview: &DiffPreview,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let (left_width, right_width) = split_widths(width);
    let mut lines: Vec<Line<'static>> = preview
        .rows
        .iter()
        .take(MAX_PREVIEW_ROWS)
        .map(|row| {
            let (left_tone, right_tone) = row.split_tones();
            let left = split_side(
                row.old.as_ref().map(|old| old.number),
                row.old.as_ref().map(|old| old.text.as_str()),
                &preview.language,
                left_tone,
                left_width,
                theme,
                highlighter,
            );
            let right = split_side(
                row.new.as_ref().map(|new| new.number),
                row.new.as_ref().map(|new| new.text.as_str()),
                &preview.language,
                right_tone,
                right_width,
                theme,
                highlighter,
            );
            join_split(left, right, theme)
        })
        .collect();
    lines.extend(truncation_notice(preview, theme));
    lines
}

fn truncation_notice(preview: &DiffPreview, theme: &Theme) -> Option<Line<'static>> {
    let hidden = preview.rows.len().checked_sub(MAX_PREVIEW_ROWS)?;
    (hidden > 0).then(|| Line::styled(format!("    … {hidden} more rows"), Style::new().fg(theme.muted)))
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
