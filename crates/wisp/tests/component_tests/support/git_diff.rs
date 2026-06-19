use std::path::PathBuf;

use wisp::git_diff::{FileDiff, FileStatus, GitDiffDocument, Hunk, PatchLine, PatchLineKind};

pub fn git_diff_document(files: Vec<FileDiff>) -> GitDiffDocument {
    GitDiffDocument { repo_root: PathBuf::from("/tmp/test"), files }
}

pub fn modified_file(path: &str, body_lines: Vec<PatchLine>) -> FileDiff {
    modified_file_with_hunks(path, vec![hunk("@@ -1,3 +1,3 @@", 1, 3, 1, 3, body_lines)])
}

pub fn modified_file_with_hunks(path: &str, hunks: Vec<Hunk>) -> FileDiff {
    FileDiff {
        old_path: Some(path.to_string()),
        path: path.to_string(),
        status: FileStatus::Modified,
        hunks,
        binary: false,
    }
}

pub fn added_file(path: &str, lines: &[&str]) -> FileDiff {
    let body_lines = lines.iter().enumerate().map(|(index, line)| added_line(*line, index + 1)).collect();
    FileDiff {
        old_path: None,
        path: path.to_string(),
        status: FileStatus::Added,
        hunks: vec![hunk(&format!("@@ -0,0 +1,{} @@", lines.len()), 0, 0, 1, lines.len(), body_lines)],
        binary: false,
    }
}

pub fn hunk(
    header: &str,
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    body_lines: Vec<PatchLine>,
) -> Hunk {
    let mut lines = vec![PatchLine {
        kind: PatchLineKind::HunkHeader,
        text: header.to_string(),
        old_line_no: None,
        new_line_no: None,
    }];
    lines.extend(body_lines);

    Hunk { header: header.to_string(), old_start, old_count, new_start, new_count, lines }
}

pub fn context_line(text: impl Into<String>, old_line_no: usize, new_line_no: usize) -> PatchLine {
    PatchLine {
        kind: PatchLineKind::Context,
        text: text.into(),
        old_line_no: Some(old_line_no),
        new_line_no: Some(new_line_no),
    }
}

pub fn removed_line(text: impl Into<String>, old_line_no: usize) -> PatchLine {
    PatchLine { kind: PatchLineKind::Removed, text: text.into(), old_line_no: Some(old_line_no), new_line_no: None }
}

pub fn added_line(text: impl Into<String>, new_line_no: usize) -> PatchLine {
    PatchLine { kind: PatchLineKind::Added, text: text.into(), old_line_no: None, new_line_no: Some(new_line_no) }
}

pub fn sample_git_diff_document() -> GitDiffDocument {
    git_diff_document(vec![
        modified_file(
            "a.rs",
            vec![
                context_line("fn main() {", 1, 1),
                removed_line("    old();", 2),
                added_line("    new();", 2),
                context_line("}", 3, 3),
            ],
        ),
        added_file("b.rs", &["new_content"]),
    ])
}

pub fn wrapping_split_document() -> GitDiffDocument {
    git_diff_document(vec![FileDiff {
        old_path: Some("x.rs".to_string()),
        path: "x.rs".to_string(),
        status: FileStatus::Modified,
        hunks: vec![hunk(
            "@@ -1,2 +1,2 @@",
            1,
            2,
            1,
            2,
            vec![
                removed_line("LEFT_MARK", 1),
                added_line(format!("RIGHT_HEAD {} RIGHT_TAIL", "A".repeat(140)), 1),
                context_line("}", 2, 2),
            ],
        )],
        binary: false,
    }])
}

pub fn comment_diff_document() -> GitDiffDocument {
    git_diff_document(vec![FileDiff {
        old_path: Some("test.rs".to_string()),
        path: "test.rs".to_string(),
        status: FileStatus::Added,
        hunks: vec![hunk(
            "@@ -0,0 +1,3 @@",
            0,
            0,
            1,
            3,
            vec![added_line("line_one", 1), added_line("line_two", 2), added_line("line_three", 3)],
        )],
        binary: false,
    }])
}
