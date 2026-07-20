use crate::git_review::{FileDiff, PatchLine, PatchLineKind};
use crate::view::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::view::wrap::fit_line;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use similar::{DiffOp, TextDiff};

/// One canonical rendered diff row: the styled line plus the patch position it
/// was laid out from, so the full-screen review can anchor comments to it and
/// the inline preview can select its content subset.
pub struct DiffRow {
    pub line: Line<'static>,
    pub hunk: usize,
    pub index: usize,
    pub kind: DiffRowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowKind {
    /// A source line: context, added, removed, or a paired split row.
    Content,
    /// A `@@` hunk header.
    HunkHeader,
    /// A metadata line that is not part of the file.
    Meta,
}

/// How a diff row is tinted. Both the inline previews in the transcript and the
/// full git-diff screen render through this, so a line looks the same wherever
/// it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTone {
    Context,
    Added,
    Removed,
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

/// A diff line rendered but not yet fitted to the viewport: callers wrap it or
/// truncate it, using `fill` for the padding and ellipsis they add.
pub struct DiffLine {
    pub line: Line<'static>,
    pub fill: Style,
}

/// Lays out `file` for `width` columns: a split view when the width allows it
/// and the diff has removals to put on the left, unified otherwise. Every diff
/// rendering in the application draws these rows or a subset of them.
pub fn diff_rows(file: &FileDiff, width: u16, theme: &Theme, highlighter: &mut SyntaxHighlighter) -> Vec<DiffRow> {
    let has_removals =
        file.hunks.iter().flat_map(|hunk| hunk.lines.iter()).any(|line| line.kind == PatchLineKind::Removed);
    if width >= SPLIT_VIEW_MIN_WIDTH && has_removals {
        split_rows(file, width, theme, highlighter)
    } else {
        unified_rows(file, width, theme, highlighter)
    }
}

/// The bounded inline preview: the first `MAX_PREVIEW_ROWS` content rows of
/// the canonical layout, with the hidden remainder collapsed into a count.
pub fn render_diff(
    file: &FileDiff,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let content: Vec<Line<'static>> = diff_rows(file, width, theme, highlighter)
        .into_iter()
        .filter(|row| row.kind == DiffRowKind::Content)
        .map(|row| row.line)
        .collect();
    let total = content.len();
    let mut lines: Vec<Line<'static>> = content.into_iter().take(MAX_PREVIEW_ROWS).collect();
    lines.extend(
        total
            .checked_sub(MAX_PREVIEW_ROWS)
            .filter(|&hidden| hidden > 0)
            .map(|hidden| Line::styled(format!("    … {hidden} more rows"), Style::new().fg(theme.muted))),
    );
    lines
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

const SPLIT_VIEW_MIN_WIDTH: u16 = 96;

/// Most rows an inline preview shows before collapsing the rest into a count.
const MAX_PREVIEW_ROWS: usize = 20;

/// The column a split view puts between its two halves.
const SPLIT_SEPARATOR: &str = "│";

fn unified_rows(file: &FileDiff, width: u16, theme: &Theme, highlighter: &mut SyntaxHighlighter) -> Vec<DiffRow> {
    file.hunks
        .iter()
        .enumerate()
        .flat_map(|(hunk, entry)| entry.lines.iter().enumerate().map(move |(index, line)| (hunk, index, line)))
        .map(|(hunk, index, line)| match line.kind {
            PatchLineKind::HunkHeader => {
                DiffRow { line: full_width(&line.text, width, theme.info), hunk, index, kind: DiffRowKind::HunkHeader }
            }
            PatchLineKind::Meta => {
                DiffRow { line: full_width(&line.text, width, theme.muted), hunk, index, kind: DiffRowKind::Meta }
            }
            kind => {
                let marker = match kind {
                    PatchLineKind::Added => '+',
                    PatchLineKind::Removed => '-',
                    _ => ' ',
                };
                let gutter = format!("{} {} {marker} ", line_number(line.old_line_no), line_number(line.new_line_no));
                let rendered = diff_line(&gutter, &line.text, file.language(), tone_of(kind), theme, highlighter);
                DiffRow {
                    line: fit_line(rendered.line, usize::from(width), rendered.fill),
                    hunk,
                    index,
                    kind: DiffRowKind::Content,
                }
            }
        })
        .collect()
}

fn split_rows(file: &FileDiff, width: u16, theme: &Theme, highlighter: &mut SyntaxHighlighter) -> Vec<DiffRow> {
    let left_width = width.saturating_sub(1) / 2;
    let right_width = width.saturating_sub(left_width + 1);
    let mut rows = Vec::new();
    for (hunk, entry) in file.hunks.iter().enumerate() {
        for group in split_groups(&entry.lines) {
            match group {
                SplitGroup::Changed { removed, added } => {
                    for (left, right) in pair_changed_block(&removed, &added) {
                        // Anchor comments on the added side when present, falling back
                        // to the removed side, so each line keeps its own comment slot.
                        let index = right.or(left).map_or(0, |side| side.index);
                        let line = split_row(
                            left.map(|side| side.line),
                            right.map(|side| side.line),
                            file.language(),
                            left_width,
                            right_width,
                            theme,
                            highlighter,
                        );
                        rows.push(DiffRow { line, hunk, index, kind: DiffRowKind::Content });
                    }
                }
                SplitGroup::Single { line, index } => rows.push(match line.kind {
                    PatchLineKind::HunkHeader => DiffRow {
                        line: full_width(&line.text, width, theme.info),
                        hunk,
                        index,
                        kind: DiffRowKind::HunkHeader,
                    },
                    // A leftover removed line has no right-hand side to pair with, and
                    // a meta line is not part of the file, so neither is content.
                    PatchLineKind::Meta | PatchLineKind::Removed => DiffRow {
                        line: full_width(&line.text, width, theme.muted),
                        hunk,
                        index,
                        kind: DiffRowKind::Meta,
                    },
                    PatchLineKind::Added | PatchLineKind::Context => {
                        let old = (line.kind == PatchLineKind::Context).then_some(line);
                        let rendered =
                            split_row(old, Some(line), file.language(), left_width, right_width, theme, highlighter);
                        DiffRow { line: rendered, hunk, index, kind: DiffRowKind::Content }
                    }
                }),
            }
        }
    }
    rows
}

fn split_row(
    old: Option<&PatchLine>,
    new: Option<&PatchLine>,
    language: &str,
    left_width: u16,
    right_width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let left = split_side(
        old.and_then(|line| line.old_line_no),
        old.map(|line| line.text.as_str()),
        language,
        old.map_or(DiffTone::Context, |line| tone_of(line.kind)),
        left_width,
        theme,
        highlighter,
    );
    let right = split_side(
        new.and_then(|line| line.new_line_no),
        new.map(|line| line.text.as_str()),
        language,
        new.map_or(DiffTone::Context, |line| tone_of(line.kind)),
        right_width,
        theme,
        highlighter,
    );
    let mut spans = left.spans;
    spans.push(Span::styled(SPLIT_SEPARATOR, Style::new().fg(theme.muted).bg(theme.background)));
    spans.extend(right.spans);
    Line::from(spans)
}

/// One half of a split diff, occupying exactly `width` columns. An absent line
/// renders as an empty, still-tinted gap so the two sides stay aligned.
fn split_side(
    number: Option<usize>,
    text: Option<&str>,
    language: &str,
    tone: DiffTone,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let gutter = format!("{} ", line_number(number));
    let rendered = diff_line(&gutter, text.unwrap_or_default(), language, tone, theme, highlighter);
    fit_line(rendered.line, usize::from(width), rendered.fill)
}

/// Syntax-highlighted spans for one source line, re-tinted to sit on
/// `background` so the highlighting does not punch holes in a diff row.
fn highlighted_spans(
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

/// Splits a hunk into the units a split view draws: a removed run followed by
/// an added run is one changed block, and everything else stands alone.
fn split_groups(lines: &[PatchLine]) -> Vec<SplitGroup<'_>> {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].kind != PatchLineKind::Removed {
            groups.push(SplitGroup::Single { line: &lines[index], index });
            index += 1;
            continue;
        }
        let sides = |range: std::ops::Range<usize>| {
            range.map(|index| SplitSide { line: &lines[index], index }).collect::<Vec<_>>()
        };
        let removed_start = index;
        index += lines[index..].iter().take_while(|line| line.kind == PatchLineKind::Removed).count();
        let added_start = index;
        index += lines[index..].iter().take_while(|line| line.kind == PatchLineKind::Added).count();
        groups
            .push(SplitGroup::Changed { removed: sides(removed_start..added_start), added: sides(added_start..index) });
    }
    groups
}

/// One unit of a split view: a standalone line, or a removed/added run drawn
/// side by side.
enum SplitGroup<'a> {
    Single { line: &'a PatchLine, index: usize },
    Changed { removed: Vec<SplitSide<'a>>, added: Vec<SplitSide<'a>> },
}

/// A patch line paired with its index into the owning hunk's `lines` vector,
/// used to preserve per-line comment anchors in the split view.
struct SplitSide<'a> {
    line: &'a PatchLine,
    index: usize,
}

/// Aligns a contiguous removed/added block using a real line-level diff so that
/// unchanged lines within the block stay paired, instead of naïvely pairing
/// `removed[i]` with `added[i]`.
fn pair_changed_block<'a>(
    removed: &'a [SplitSide<'a>],
    added: &'a [SplitSide<'a>],
) -> Vec<(Option<&'a SplitSide<'a>>, Option<&'a SplitSide<'a>>)> {
    let old: Vec<&str> = removed.iter().map(|side| side.line.text.as_str()).collect();
    let new: Vec<&str> = added.iter().map(|side| side.line.text.as_str()).collect();
    let diff = TextDiff::from_slices(&old, &new);
    let mut pairs = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { old_index, new_index, len } => {
                for offset in 0..len {
                    pairs.push((Some(&removed[old_index + offset]), Some(&added[new_index + offset])));
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                pairs.extend(removed[old_index..old_index + old_len].iter().map(|side| (Some(side), None)));
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                pairs.extend(added[new_index..new_index + new_len].iter().map(|side| (None, Some(side))));
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let pair_len = old_len.min(new_len);
                for offset in 0..pair_len {
                    pairs.push((Some(&removed[old_index + offset]), Some(&added[new_index + offset])));
                }
                pairs.extend(removed[old_index + pair_len..old_index + old_len].iter().map(|side| (Some(side), None)));
                pairs.extend(added[new_index + pair_len..new_index + new_len].iter().map(|side| (None, Some(side))));
            }
        }
    }
    pairs
}

fn tone_of(kind: PatchLineKind) -> DiffTone {
    match kind {
        PatchLineKind::Added => DiffTone::Added,
        PatchLineKind::Removed => DiffTone::Removed,
        _ => DiffTone::Context,
    }
}

fn line_number(number: Option<usize>) -> String {
    number.map_or_else(|| "    ".to_string(), |number| format!("{number:>4}"))
}

fn full_width(text: &str, width: u16, color: Color) -> Line<'static> {
    let style = Style::new().fg(color);
    fit_line(Line::styled(text.to_string(), style), usize::from(width), style)
}

