use similar::{DiffOp, TextDiff};

/// Tag indicating the kind of change a diff line represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Context,
    Removed,
    Added,
}

/// A single line in a diff, tagged with its change type.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub tag: DiffTag,
    pub content: String,
}

/// A row in a side-by-side diff, pairing an old (left) line with a new (right) line.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitDiffRow {
    pub left: Option<SplitDiffEntry>,
    pub right: Option<SplitDiffEntry>,
}

/// One side of a split diff row.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitDiffEntry {
    pub tag: DiffTag,
    pub content: String,
    pub line_number: Option<usize>,
}

impl SplitDiffEntry {
    pub fn new(tag: DiffTag, content: impl Into<String>, line_number: Option<usize>) -> Self {
        Self { tag, content: content.into(), line_number }
    }
}

/// A preview of changed lines for an edit operation.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffPreview {
    /// Flat list of diff lines — used by the unified renderer.
    pub lines: Vec<DiffLine>,
    /// Paired rows — used by the split (side-by-side) renderer.
    pub rows: Vec<SplitDiffRow>,
    pub lang_hint: String,
    /// 1-indexed line number where the edit begins in the original file.
    pub start_line: Option<usize>,
}

impl DiffPreview {
    pub fn compute(old: &str, new: &str, lang_hint: &str) -> Self {
        build_diff(old, new, lang_hint, false)
    }

    pub fn compute_trimmed(old: &str, new: &str, lang_hint: &str) -> Self {
        build_diff(old, new, lang_hint, true)
    }
}

fn build_diff(old: &str, new: &str, lang_hint: &str, trim: bool) -> DiffPreview {
    let text_diff = TextDiff::from_lines(old, new);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut state = DiffBuildState::default();

    for op in text_diff.ops() {
        process_diff_op(*op, &old_lines, &new_lines, &mut state);
    }

    let DiffBuildState { mut lines, mut rows, mut first_change_line, .. } = state;
    if trim {
        trim_context(&mut lines, &mut rows, &mut first_change_line);
    }

    DiffPreview { lines, rows, lang_hint: lang_hint.to_string(), start_line: first_change_line }
}

#[derive(Default)]
struct DiffBuildState {
    lines: Vec<DiffLine>,
    rows: Vec<SplitDiffRow>,
    first_change_line: Option<usize>,
    old_line_num: usize,
    new_line_num: usize,
}

fn get_line<'a>(lines: &[&'a str], index: usize) -> &'a str {
    lines.get(index).unwrap_or(&"").trim_end_matches('\n')
}

fn process_diff_op(op: DiffOp, old: &[&str], new: &[&str], s: &mut DiffBuildState) {
    match op {
        DiffOp::Equal { old_index, len, .. } => {
            for i in 0..len {
                s.old_line_num += 1;
                s.new_line_num += 1;
                let content = get_line(old, old_index + i).to_string();
                s.lines.push(DiffLine { tag: DiffTag::Context, content: content.clone() });
                s.rows.push(SplitDiffRow {
                    left: Some(SplitDiffEntry::new(DiffTag::Context, content.clone(), Some(s.old_line_num))),
                    right: Some(SplitDiffEntry::new(DiffTag::Context, content, Some(s.new_line_num))),
                });
            }
        }
        DiffOp::Delete { old_index, old_len, .. } => {
            if s.first_change_line.is_none() {
                s.first_change_line = Some(s.old_line_num + 1);
            }
            for i in 0..old_len {
                s.old_line_num += 1;
                let content = get_line(old, old_index + i).to_string();
                s.lines.push(DiffLine { tag: DiffTag::Removed, content: content.clone() });
                s.rows.push(SplitDiffRow {
                    left: Some(SplitDiffEntry::new(DiffTag::Removed, content, Some(s.old_line_num))),
                    right: None,
                });
            }
        }
        DiffOp::Insert { new_index, new_len, .. } => {
            if s.first_change_line.is_none() {
                s.first_change_line = Some(s.old_line_num + 1);
            }
            for i in 0..new_len {
                s.new_line_num += 1;
                let content = get_line(new, new_index + i).to_string();
                s.lines.push(DiffLine { tag: DiffTag::Added, content: content.clone() });
                s.rows.push(SplitDiffRow {
                    left: None,
                    right: Some(SplitDiffEntry::new(DiffTag::Added, content, Some(s.new_line_num))),
                });
            }
        }
        DiffOp::Replace { old_index, old_len, new_index, new_len } => {
            if s.first_change_line.is_none() {
                s.first_change_line = Some(s.old_line_num + 1);
            }
            s.lines.extend(
                (0..old_len)
                    .map(|i| DiffLine { tag: DiffTag::Removed, content: get_line(old, old_index + i).to_string() }),
            );
            s.lines.extend(
                (0..new_len)
                    .map(|i| DiffLine { tag: DiffTag::Added, content: get_line(new, new_index + i).to_string() }),
            );
            for i in 0..old_len.max(new_len) {
                let left = (i < old_len).then(|| {
                    s.old_line_num += 1;
                    SplitDiffEntry::new(DiffTag::Removed, get_line(old, old_index + i), Some(s.old_line_num))
                });
                let right = (i < new_len).then(|| {
                    s.new_line_num += 1;
                    SplitDiffEntry::new(DiffTag::Added, get_line(new, new_index + i), Some(s.new_line_num))
                });
                s.rows.push(SplitDiffRow { left, right });
            }
        }
    }
}

fn trim_context(lines: &mut Vec<DiffLine>, rows: &mut Vec<SplitDiffRow>, first_change_line: &mut Option<usize>) {
    const CONTEXT_LINES: usize = 3;

    let first_change_idx = lines.iter().position(|l| l.tag != DiffTag::Context);
    let last_change_idx = lines.iter().rposition(|l| l.tag != DiffTag::Context);

    if let (Some(first), Some(last)) = (first_change_idx, last_change_idx) {
        let start = first.saturating_sub(CONTEXT_LINES);
        let end = (last + CONTEXT_LINES + 1).min(lines.len());
        lines.drain(..start);
        lines.truncate(end - start);
        let trimmed_context = first - start;
        *first_change_line = first_change_line.map(|l| l - trimmed_context);
    }

    let first_row = rows.iter().position(|r| !is_context_row(r));
    let last_row = rows.iter().rposition(|r| !is_context_row(r));

    if let (Some(first), Some(last)) = (first_row, last_row) {
        let start = first.saturating_sub(CONTEXT_LINES);
        let end = (last + CONTEXT_LINES + 1).min(rows.len());
        rows.drain(..start);
        rows.truncate(end - start);
    }
}

fn is_context_row(row: &SplitDiffRow) -> bool {
    row.left.as_ref().is_none_or(|c| c.tag == DiffTag::Context)
        && row.right.as_ref().is_none_or(|c| c.tag == DiffTag::Context)
}
