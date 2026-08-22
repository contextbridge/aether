use std::path::PathBuf;

use wisp::git_diff::{FileDiff, FileStatus, GitDiffDocument, Hunk, PatchLine, StageState};

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
        staged: StageState::Unstaged,
        hunks,
        binary: false,
    }
}

pub fn added_file(path: &str, lines: &[&str]) -> FileDiff {
    let body_lines = lines.iter().enumerate().map(|(index, line)| PatchLine::added(*line, index + 1)).collect();
    FileDiff {
        old_path: None,
        path: path.to_string(),
        status: FileStatus::Added,
        staged: StageState::Unstaged,
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
    let mut lines = vec![PatchLine::hunk_header(header)];
    lines.extend(body_lines);

    Hunk { header: header.to_string(), old_start, old_count, new_start, new_count, lines }
}

pub fn sample_git_diff_document() -> GitDiffDocument {
    git_diff_document(vec![
        modified_file(
            "a.rs",
            vec![
                PatchLine::context("fn main() {", 1, 1),
                PatchLine::removed("    old();", 2),
                PatchLine::added("    new();", 2),
                PatchLine::context("}", 3, 3),
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
        staged: StageState::Unstaged,
        hunks: vec![hunk(
            "@@ -1,2 +1,2 @@",
            1,
            2,
            1,
            2,
            vec![
                PatchLine::removed("LEFT_MARK", 1),
                PatchLine::added(format!("RIGHT_HEAD {} RIGHT_TAIL", "A".repeat(140)), 1),
                PatchLine::context("}", 2, 2),
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
        staged: StageState::Unstaged,
        hunks: vec![hunk(
            "@@ -0,0 +1,3 @@",
            0,
            0,
            1,
            3,
            vec![PatchLine::added("line_one", 1), PatchLine::added("line_two", 2), PatchLine::added("line_three", 3)],
        )],
        binary: false,
    }])
}
