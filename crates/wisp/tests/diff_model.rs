use std::path::PathBuf;

use wisp::git_review::{
    DiffDocument, DiffScope, FileDiff, FileStatus, PatchLineKind, StageState, parse_porcelain_status,
    parse_unified_diff,
};

#[test]
fn acp_texts_normalize_into_canonical_numbered_lines() {
    let file = FileDiff::from_texts("src/lib.rs", "before\nkeep\n", "after\nkeep\nnew\n");

    assert_eq!(file.status, FileStatus::Modified);
    assert_eq!(file.old_path.as_deref(), Some("src/lib.rs"));
    let lines = &file.hunks[0].lines;
    assert_eq!(lines[0].kind, PatchLineKind::HunkHeader);
    assert_eq!(lines[1].old_line_no, Some(1));
    assert_eq!(lines[1].new_line_no, None);
    assert_eq!(lines[1].kind, PatchLineKind::Removed);
    assert_eq!(lines[2].old_line_no, None);
    assert_eq!(lines[2].new_line_no, Some(1));
    assert_eq!(lines[2].kind, PatchLineKind::Added);
    assert_eq!(lines.last().and_then(|line| line.new_line_no), Some(3));
}

#[test]
fn acp_texts_cover_added_and_deleted_files() {
    let added = FileDiff::from_texts("src/new.rs", "", "a\nb\n");
    assert_eq!(added.status, FileStatus::Added);
    assert_eq!(added.old_path, None);
    assert_eq!(added.hunks.len(), 1);
    assert_eq!(added.hunks[0].header, "@@ -0,0 +1,2 @@");
    assert_eq!(added.hunks[0].lines.iter().filter(|line| line.kind == PatchLineKind::Added).count(), 2);

    let deleted = FileDiff::from_texts("src/gone.rs", "a\nb\n", "");
    assert_eq!(deleted.status, FileStatus::Deleted);
    assert_eq!(deleted.hunks.len(), 1);
    assert_eq!(deleted.hunks[0].header, "@@ -1,2 +0,0 @@");
    assert_eq!(deleted.hunks[0].old_count, 2);
    assert_eq!(deleted.hunks[0].new_count, 0);

    let unchanged = FileDiff::from_texts("src/same.rs", "a\nb\n", "a\nb\n");
    assert_eq!(unchanged.status, FileStatus::Modified);
    assert!(unchanged.hunks.is_empty());
}

#[test]
fn acp_texts_split_distant_changes_into_separate_hunks() {
    let old = (1..=20).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
    let new = old
        .lines()
        .enumerate()
        .map(|(index, line)| if index == 1 || index == 18 { format!("{line} changed") } else { line.to_string() })
        .collect::<Vec<_>>()
        .join("\n");

    let file = FileDiff::from_texts("src/lib.rs", &old, &new);

    assert_eq!(file.hunks.len(), 2, "distant changes must not be merged into one hunk");
    for hunk in &file.hunks {
        let context = hunk.lines.iter().filter(|line| line.kind == PatchLineKind::Context).count();
        assert!(context <= 6, "each hunk carries at most 3 context lines per side, got {context}");
        assert!(hunk.lines.first().is_some_and(|line| line.kind == PatchLineKind::HunkHeader));
    }
    assert_eq!(file.hunks[0].old_start, 1);
    assert_eq!(file.hunks[1].old_start, 16);
}

#[test]
fn git_output_normalizes_rename_binary_and_untracked_files() {
    let diff = concat!(
        "diff --git a/old.txt b/new.txt\n",
        "similarity index 100%\n",
        "rename from old.txt\n",
        "rename to new.txt\n",
        "diff --git a/image.png b/image.png\n",
        "index 123..456\n",
        "Binary files a/image.png and b/image.png differ\n",
    );
    let document = DiffDocument::from_git_output(
        PathBuf::from("/repo"),
        diff,
        "R  old.txt\0new.txt\0?? notes.txt\0",
        [("notes.txt".to_string(), b"hello\n".to_vec())],
        DiffScope::Both,
    )
    .unwrap();

    assert_eq!(document.files.len(), 3);
    assert_eq!(document.files[0].path, "image.png");
    assert!(document.files[0].binary);
    assert_eq!(document.files[1].status, FileStatus::Renamed);
    assert_eq!(document.files[1].old_path.as_deref(), Some("old.txt"));
    assert_eq!(document.files[1].staged, StageState::Staged);
    assert_eq!(document.files[2].status, FileStatus::Untracked);
    assert_eq!(document.files[2].staged, StageState::Unstaged);
}

#[test]
fn porcelain_status_distinguishes_staged_partial_and_unstaged() {
    let status = parse_porcelain_status("M  staged.rs\0MM partial.rs\0 M unstaged.rs\0");

    assert_eq!(status["staged.rs"], StageState::Staged);
    assert_eq!(status["partial.rs"], StageState::PartiallyStaged);
    assert_eq!(status["unstaged.rs"], StageState::Unstaged);
}

#[test]
fn unified_parser_accepts_single_line_hunk_ranges() {
    let files = parse_unified_diff(concat!(
        "diff --git a/a.txt b/a.txt\n",
        "index 111..222\n",
        "--- a/a.txt\n",
        "+++ b/a.txt\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
    ))
    .unwrap();

    assert_eq!(files[0].hunks[0].old_count, 1);
    assert_eq!(files[0].hunks[0].new_count, 1);
    assert_eq!(files[0].hunks[0].lines[1].text, "old");
}
