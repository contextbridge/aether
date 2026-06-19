use super::patch_renderer::{RenderedPatch, lang_hint_from_path};
use super::{DiffAnchor, PatchAnchor};
use crate::components::common::AnchoredSurfaceBuilder;
use crate::components::review_comments::CommentAnchor;
use crate::git_diff::{FileDiff, PatchLine, PatchLineKind};
use tui::{DiffTag, Frame, FramePart, GutterTint, Line, SplitDiffEntry, ViewContext};

const SEPARATOR_WIDTH_U16: u16 = 1;

pub fn build_split_patch_base_lines(file: &FileDiff, width: usize, ctx: &ViewContext) -> RenderedPatch {
    let theme = &ctx.theme;
    let lang_hint = lang_hint_from_path(&file.path);
    let max_line_no = file.max_line_no();
    let metrics = tui::SplitLayoutDimensions::new(width, max_line_no);

    let mut rows: AnchoredSurfaceBuilder<PatchAnchor> = AnchoredSurfaceBuilder::new();

    for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
        for row in pair_hunk_lines(&hunk.lines) {
            let anchor = row.anchor(hunk_idx);

            match row {
                PairedRow::Meta { text, .. } => {
                    let line = Line::with_style(text, tui::Style::fg(theme.text_secondary()).italic());

                    if let Some(anchor) = anchor {
                        rows.push_raw_anchored_rows(anchor, [line]);
                    } else {
                        rows.push_raw_unanchored_rows([line]);
                    }
                }
                PairedRow::Split { left, right } => {
                    let left_entry = left.as_ref().map(|side| {
                        SplitDiffEntry::new(
                            match side.kind {
                                PatchLineKind::Removed => DiffTag::Removed,
                                _ => DiffTag::Context,
                            },
                            side.text,
                            side.line_no,
                        )
                    });
                    let right_entry = right.as_ref().map(|side| {
                        SplitDiffEntry::new(
                            match side.kind {
                                PatchLineKind::Added => DiffTag::Added,
                                _ => DiffTag::Context,
                            },
                            side.text,
                            side.line_no,
                        )
                    });

                    let left_frame = tui::split_render_entry(
                        left_entry.as_ref(),
                        metrics.left_content_width,
                        lang_hint,
                        tui::SplitDiffSide::Left,
                        metrics.gutter_width,
                        GutterTint::Diff,
                        ctx,
                    );
                    let right_frame = tui::split_render_entry(
                        right_entry.as_ref(),
                        metrics.right_content_width,
                        lang_hint,
                        tui::SplitDiffSide::Right,
                        metrics.gutter_width,
                        GutterTint::Diff,
                        ctx,
                    );
                    let height = left_frame.lines().len().max(right_frame.lines().len());

                    let sep_line = Line::new(tui::SEPARATOR.to_string());
                    let sep_frame = Frame::new(vec![sep_line; height]);
                    let row_frame = Frame::hstack([
                        FramePart::new(left_frame, metrics.left_panel_width),
                        FramePart::new(sep_frame, SEPARATOR_WIDTH_U16),
                        FramePart::new(right_frame, metrics.right_panel_width),
                    ]);

                    if let Some(anchor) = anchor {
                        rows.push_raw_anchored_rows(anchor, row_frame.into_lines());
                    } else {
                        rows.push_raw_unanchored_rows(row_frame.into_lines());
                    }
                }
            }
        }
    }

    RenderedPatch::new(rows.finish())
}

#[derive(Clone, Copy)]
struct SideInfo<'a> {
    kind: PatchLineKind,
    text: &'a str,
    line_no: Option<usize>,
    line_idx: usize,
}

enum PairedRow<'a> {
    Meta { line_idx: usize, text: &'a str },
    Split { left: Option<SideInfo<'a>>, right: Option<SideInfo<'a>> },
}

impl PairedRow<'_> {
    fn anchor(&self, hunk_index: usize) -> Option<DiffAnchor> {
        match self {
            Self::Meta { line_idx, .. } => Some(CommentAnchor(PatchAnchor { hunk: hunk_index, line: *line_idx })),
            Self::Split { left, right } => {
                right.as_ref().map(|side| CommentAnchor(PatchAnchor { hunk: hunk_index, line: side.line_idx })).or_else(
                    || left.as_ref().map(|side| CommentAnchor(PatchAnchor { hunk: hunk_index, line: side.line_idx })),
                )
            }
        }
    }
}

fn pair_hunk_lines(lines: &[PatchLine]) -> Vec<PairedRow<'_>> {
    let mut rows = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let patch_line = &lines[index];
        match patch_line.kind {
            PatchLineKind::HunkHeader => {
                index += 1;
            }
            PatchLineKind::Meta => {
                rows.push(PairedRow::Meta { line_idx: index, text: &patch_line.text });
                index += 1;
            }
            PatchLineKind::Context => {
                rows.push(PairedRow::Split {
                    left: Some(SideInfo {
                        kind: patch_line.kind,
                        text: &patch_line.text,
                        line_no: patch_line.old_line_no,
                        line_idx: index,
                    }),
                    right: Some(SideInfo {
                        kind: patch_line.kind,
                        text: &patch_line.text,
                        line_no: patch_line.new_line_no,
                        line_idx: index,
                    }),
                });
                index += 1;
            }
            PatchLineKind::Removed => {
                let mut removed = Vec::new();
                while index < lines.len() && lines[index].kind == PatchLineKind::Removed {
                    removed.push(side_info(&lines[index], index));
                    index += 1;
                }

                let mut added = Vec::new();
                while index < lines.len() && lines[index].kind == PatchLineKind::Added {
                    added.push(side_info(&lines[index], index));
                    index += 1;
                }

                rows.extend(pair_changed_block(&removed, &added));
            }
            PatchLineKind::Added => {
                let mut added = Vec::new();
                while index < lines.len() && lines[index].kind == PatchLineKind::Added {
                    added.push(side_info(&lines[index], index));
                    index += 1;
                }
                rows.extend(pair_changed_block(&[], &added));
            }
        }
    }

    rows
}

fn side_info(line: &PatchLine, line_idx: usize) -> SideInfo<'_> {
    let line_no = match line.kind {
        PatchLineKind::Added => line.new_line_no,
        PatchLineKind::Removed | PatchLineKind::Context => line.old_line_no,
        PatchLineKind::HunkHeader | PatchLineKind::Meta => None,
    };

    SideInfo { kind: line.kind, text: &line.text, line_no, line_idx }
}

fn pair_changed_block<'a>(removed: &[SideInfo<'a>], added: &[SideInfo<'a>]) -> Vec<PairedRow<'a>> {
    let old: Vec<&str> = removed.iter().map(|side| side.text).collect();
    let new: Vec<&str> = added.iter().map(|side| side.text).collect();
    let diff = similar::TextDiff::from_slices(&old, &new);
    let mut rows = Vec::new();

    for op in diff.ops() {
        match *op {
            similar::DiffOp::Equal { old_index, new_index, len } => {
                for offset in 0..len {
                    rows.push(split_row(Some(removed[old_index + offset]), Some(added[new_index + offset])));
                }
            }
            similar::DiffOp::Delete { old_index, old_len, .. } => {
                for side in &removed[old_index..old_index + old_len] {
                    rows.push(split_row(Some(*side), None));
                }
            }
            similar::DiffOp::Insert { new_index, new_len, .. } => {
                for side in &added[new_index..new_index + new_len] {
                    rows.push(split_row(None, Some(*side)));
                }
            }
            similar::DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let pair_len = old_len.min(new_len);

                for offset in 0..pair_len {
                    rows.push(split_row(Some(removed[old_index + offset]), Some(added[new_index + offset])));
                }

                for side in &removed[old_index + pair_len..old_index + old_len] {
                    rows.push(split_row(Some(*side), None));
                }
                for side in &added[new_index + pair_len..new_index + new_len] {
                    rows.push(split_row(None, Some(*side)));
                }
            }
        }
    }

    rows
}

fn split_row<'a>(left: Option<SideInfo<'a>>, right: Option<SideInfo<'a>>) -> PairedRow<'a> {
    PairedRow::Split { left, right }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff::{FileStatus, Hunk};

    fn pl(kind: PatchLineKind, text: &str, old: Option<usize>, new: Option<usize>) -> PatchLine {
        PatchLine { kind, text: text.to_string(), old_line_no: old, new_line_no: new }
    }

    fn test_file(hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            old_path: Some("test.rs".to_string()),
            path: "test.rs".to_string(),
            status: FileStatus::Modified,
            hunks,
            binary: false,
        }
    }

    fn test_hunk(lines: Vec<PatchLine>) -> Hunk {
        Hunk { header: "@@ -1,3 +1,3 @@".to_string(), old_start: 1, old_count: 3, new_start: 1, new_count: 3, lines }
    }

    fn ctx() -> ViewContext {
        ViewContext::new((120, 24))
    }

    #[test]
    fn split_base_lines_has_insert_row_lookup() {
        let file = test_file(vec![test_hunk(vec![
            pl(PatchLineKind::HunkHeader, "@@ -1,3 +1,3 @@", None, None),
            pl(PatchLineKind::Removed, "old_a", Some(1), None),
            pl(PatchLineKind::Added, "new_a", None, Some(1)),
        ])]);
        let result = build_split_patch_base_lines(&file, 100, &ctx());

        assert_eq!(result.surface.groups().len(), 1);
        assert_eq!(result.surface.end_row_for_anchor(CommentAnchor(PatchAnchor { hunk: 0, line: 2 })), Some(0));
    }

    #[test]
    fn split_pairs_removed_and_added() {
        let file = test_file(vec![test_hunk(vec![
            pl(PatchLineKind::HunkHeader, "@@ -1,3 +1,3 @@", None, None),
            pl(PatchLineKind::Removed, "old_a", Some(1), None),
            pl(PatchLineKind::Removed, "old_b", Some(2), None),
            pl(PatchLineKind::Added, "new_a", None, Some(1)),
            pl(PatchLineKind::Added, "new_b", None, Some(2)),
        ])]);
        let result = build_split_patch_base_lines(&file, 100, &ctx());

        assert_eq!(result.surface.lines().len(), 2);
        assert_eq!(result.surface.groups().len(), 2);
        assert_eq!(result.surface.start_row_for_anchor(CommentAnchor(PatchAnchor { hunk: 0, line: 3 })), Some(0));
        assert_eq!(result.surface.start_row_for_anchor(CommentAnchor(PatchAnchor { hunk: 0, line: 4 })), Some(1));
    }

    #[test]
    fn split_refs_prefer_right_side() {
        let file = test_file(vec![test_hunk(vec![
            pl(PatchLineKind::HunkHeader, "@@ -1,1 +1,1 @@", None, None),
            pl(PatchLineKind::Removed, "old", Some(1), None),
            pl(PatchLineKind::Added, "new", None, Some(1)),
        ])]);
        let result = build_split_patch_base_lines(&file, 100, &ctx());

        let CommentAnchor(PatchAnchor { line, .. }) =
            result.surface.anchor_at_or_before(0).expect("split row should have an anchor");
        assert_eq!(line, 2, "should reference the Added line");
    }

    #[test]
    fn moved_identical_line_is_aligned_on_same_row() {
        let file = test_file(vec![test_hunk(vec![
            pl(PatchLineKind::HunkHeader, "@@ -1,2 +1,2 @@", None, None),
            pl(PatchLineKind::Removed, "let before = old();", Some(1), None),
            pl(PatchLineKind::Removed, "shared_call();", Some(2), None),
            pl(PatchLineKind::Added, "shared_call();", None, Some(1)),
            pl(PatchLineKind::Added, "let after = new();", None, Some(2)),
        ])]);
        let result = build_split_patch_base_lines(&file, 100, &ctx());

        let shared_row =
            result.surface.lines().iter().find(|line| line.plain_text().matches("shared_call();").count() == 2);
        assert!(shared_row.is_some(), "identical moved lines should be aligned onto the same split row");
    }

    #[test]
    fn replace_with_extra_added_line_keeps_prefix_aligned_and_overflow_unpaired() {
        let file = test_file(vec![test_hunk(vec![
            pl(PatchLineKind::HunkHeader, "@@ -1,2 +1,3 @@", None, None),
            pl(PatchLineKind::Removed, "shared_call();", Some(1), None),
            pl(PatchLineKind::Removed, "let old_value = foo();", Some(2), None),
            pl(PatchLineKind::Added, "shared_call();", None, Some(1)),
            pl(PatchLineKind::Added, "let new_value = bar();", None, Some(2)),
            pl(PatchLineKind::Added, "extra_call();", None, Some(3)),
        ])]);
        let result = build_split_patch_base_lines(&file, 100, &ctx());

        let shared_row =
            result.surface.lines().iter().find(|line| line.plain_text().matches("shared_call();").count() == 2);
        assert!(shared_row.is_some(), "shared prefix line should stay aligned as an unchanged pair");

        let overflow_row = result
            .surface
            .lines()
            .iter()
            .skip(1)
            .find(|line| line.plain_text().contains("extra_call();"))
            .expect("expected overflow added row");
        assert_eq!(
            overflow_row.plain_text().matches("extra_call();").count(),
            1,
            "overflow added line should remain unpaired"
        );
    }
}
