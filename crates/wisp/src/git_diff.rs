use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[doc = include_str!("docs/git_diff_document.md")]
#[allow(dead_code)]
pub struct GitDiffDocument {
    pub repo_root: PathBuf,
    pub files: Vec<FileDiff>,
}

pub struct FileDiff {
    pub old_path: Option<String>,
    pub path: String,
    pub status: FileStatus,
    pub staged: StageState,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffScope {
    Unstaged,
    Staged,
    #[default]
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Unstaged,
    Staged,
    PartiallyStaged,
}

#[allow(dead_code)]
pub struct Hunk {
    pub header: String,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<PatchLine>,
}

pub struct PatchLine {
    pub kind: PatchLineKind,
    pub text: String,
    pub old_line_no: Option<usize>,
    pub new_line_no: Option<usize>,
}

impl PatchLine {
    pub fn hunk_header(header: &str) -> Self {
        Self { kind: PatchLineKind::HunkHeader, text: header.to_string(), old_line_no: None, new_line_no: None }
    }

    pub fn context(text: impl Into<String>, old_line_no: usize, new_line_no: usize) -> Self {
        Self {
            kind: PatchLineKind::Context,
            text: text.into(),
            old_line_no: Some(old_line_no),
            new_line_no: Some(new_line_no),
        }
    }

    pub fn added(text: impl Into<String>, new_line_no: usize) -> Self {
        Self { kind: PatchLineKind::Added, text: text.into(), old_line_no: None, new_line_no: Some(new_line_no) }
    }

    pub fn removed(text: impl Into<String>, old_line_no: usize) -> Self {
        Self { kind: PatchLineKind::Removed, text: text.into(), old_line_no: Some(old_line_no), new_line_no: None }
    }

    pub fn meta(text: impl Into<String>) -> Self {
        Self { kind: PatchLineKind::Meta, text: text.into(), old_line_no: None, new_line_no: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchLineKind {
    HunkHeader,
    Context,
    Added,
    Removed,
    Meta,
}

#[doc = include_str!("docs/git_diff_error.md")]
#[derive(Debug, Error)]
pub enum GitDiffError {
    #[error("Not a git repository")]
    NotARepository,
    #[error("Git command failed: {stderr}")]
    CommandFailed { stderr: String },
    #[error("Failed to parse diff: {0}")]
    ParseError(String),
}

impl DiffScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
            Self::Both => "Both",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Both => Self::Unstaged,
            Self::Unstaged => Self::Staged,
            Self::Staged => Self::Both,
        }
    }

    fn includes_untracked(self) -> bool {
        !matches!(self, Self::Staged)
    }
}

impl FileStatus {
    pub fn marker(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Untracked => '?',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "new file",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Untracked => "untracked",
        }
    }
}

impl FileDiff {
    pub fn additions(&self) -> usize {
        self.hunks.iter().map(Hunk::additions).sum()
    }

    pub fn deletions(&self) -> usize {
        self.hunks.iter().map(Hunk::deletions).sum()
    }

    pub fn max_line_no(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .flat_map(|line| line.old_line_no.into_iter().chain(line.new_line_no))
            .max()
            .unwrap_or(0)
    }
}

impl Hunk {
    pub fn additions(&self) -> usize {
        self.lines.iter().filter(|line| line.kind == PatchLineKind::Added).count()
    }

    pub fn deletions(&self) -> usize {
        self.lines.iter().filter(|line| line.kind == PatchLineKind::Removed).count()
    }
}

pub(crate) async fn load_git_diff(
    working_dir: &Path,
    cached_repo_root: Option<&Path>,
    scope: DiffScope,
) -> Result<GitDiffDocument, GitDiffError> {
    let repo_root = match cached_repo_root {
        Some(root) => root.to_path_buf(),
        None => resolve_repo_root(working_dir).await?,
    };
    let diff_args = match scope {
        DiffScope::Unstaged => &["diff", "--no-ext-diff", "--find-renames"][..],
        DiffScope::Staged => &["diff", "--cached", "--no-ext-diff", "--find-renames"][..],
        DiffScope::Both => &["diff", "--no-ext-diff", "--find-renames", "HEAD"][..],
    };
    let diff_output = git(&repo_root, diff_args).await?;

    let mut files = if diff_output.trim().is_empty() { Vec::new() } else { parse_unified_diff(&diff_output)? };

    if scope.includes_untracked() {
        let untracked_stdout = git(&repo_root, &["ls-files", "--others", "--exclude-standard"]).await?;
        for path in untracked_stdout.lines().filter(|l| !l.is_empty()).map(String::from) {
            files.push(build_untracked_file_diff(&repo_root, path).await);
        }
    }

    let status_map = load_status_map(&repo_root).await?;
    for file in &mut files {
        file.staged = status_map.get(&file.path).copied().unwrap_or(StageState::Unstaged);
    }

    Ok(GitDiffDocument { repo_root, files })
}

pub(crate) async fn stage_files(repo_root: &Path, paths: &[&str]) -> Result<(), GitDiffError> {
    let mut args = vec!["add", "--"];
    args.extend_from_slice(paths);
    git(repo_root, &args).await.map(drop)
}

pub(crate) async fn unstage_files(repo_root: &Path, paths: &[&str]) -> Result<(), GitDiffError> {
    let mut args = vec!["reset", "--quiet", "--"];
    args.extend_from_slice(paths);
    git(repo_root, &args).await.map(drop)
}

pub(crate) async fn stage_all(repo_root: &Path) -> Result<(), GitDiffError> {
    git(repo_root, &["add", "-A"]).await.map(drop)
}

pub(crate) async fn unstage_all(repo_root: &Path) -> Result<(), GitDiffError> {
    git(repo_root, &["reset", "--quiet"]).await.map(drop)
}

pub(crate) async fn discard_file(repo_root: &Path, path: &str, status: FileStatus) -> Result<(), GitDiffError> {
    match status {
        FileStatus::Untracked => git(repo_root, &["clean", "-f", "--", path]).await.map(drop),
        _ => git(repo_root, &["restore", "--source=HEAD", "--staged", "--worktree", "--", path]).await.map(drop),
    }
}

pub(crate) async fn commit(repo_root: &Path, message: &str) -> Result<(), GitDiffError> {
    if message.trim().is_empty() {
        return Err(GitDiffError::CommandFailed { stderr: "empty commit message".to_string() });
    }
    git(repo_root, &["commit", "-m", message]).await.map(drop)
}

async fn load_status_map(repo_root: &Path) -> Result<HashMap<String, StageState>, GitDiffError> {
    let output = git(repo_root, &["status", "--porcelain=v1", "-z"]).await?;
    Ok(parse_porcelain_status(&output))
}

fn parse_porcelain_status(input: &str) -> HashMap<String, StageState> {
    let mut map = HashMap::new();
    let mut tokens = input.split('\0').filter(|token| !token.is_empty());

    while let Some(record) = tokens.next() {
        if record.len() < 3 {
            continue;
        }
        let bytes = record.as_bytes();
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        let path = &record[3..];

        if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            tokens.next();
        }

        let state = match (index, worktree) {
            ('?', '?') | (' ', _) => StageState::Unstaged,
            (_, ' ') => StageState::Staged,
            _ => StageState::PartiallyStaged,
        };
        map.insert(path.to_string(), state);
    }

    map
}

async fn resolve_repo_root(working_dir: &Path) -> Result<PathBuf, GitDiffError> {
    let output = tokio::process::Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| GitDiffError::CommandFailed { stderr: e.to_string() })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Err(GitDiffError::NotARepository);
        }
        return Err(GitDiffError::CommandFailed { stderr: stderr.into_owned() });
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}

async fn git(repo_root: &Path, args: &[&str]) -> Result<String, GitDiffError> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| GitDiffError::CommandFailed { stderr: e.to_string() })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitDiffError::CommandFailed { stderr: stderr.into_owned() });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn build_untracked_file_diff(repo_root: &Path, relative_path: String) -> FileDiff {
    let full_path = repo_root.join(&relative_path);
    let Ok(bytes) = tokio::fs::read(&full_path).await else {
        return binary_untracked(relative_path);
    };

    if bytes.iter().take(8192).any(|&b| b == 0) {
        return binary_untracked(relative_path);
    }

    let Ok(content) = String::from_utf8(bytes) else {
        return binary_untracked(relative_path);
    };

    let text_lines: Vec<&str> = content.lines().collect();
    let line_count = text_lines.len();

    let hunk_header = format!("@@ -0,0 +1,{line_count} @@");

    let mut patch_lines = vec![PatchLine::hunk_header(&hunk_header)];

    for (i, line) in text_lines.iter().enumerate() {
        patch_lines.push(PatchLine::added(*line, i + 1));
    }

    let hunk = Hunk {
        header: hunk_header,
        old_start: 0,
        old_count: 0,
        new_start: 1,
        new_count: line_count,
        lines: patch_lines,
    };

    FileDiff {
        old_path: None,
        path: relative_path,
        status: FileStatus::Untracked,
        staged: StageState::Unstaged,
        hunks: vec![hunk],
        binary: false,
    }
}

fn binary_untracked(path: String) -> FileDiff {
    FileDiff {
        old_path: None,
        path,
        status: FileStatus::Untracked,
        staged: StageState::Unstaged,
        hunks: Vec::new(),
        binary: true,
    }
}

pub(crate) fn parse_unified_diff(input: &str) -> Result<Vec<FileDiff>, GitDiffError> {
    split_diff_files(input).into_iter().map(parse_file_diff).collect()
}

fn split_diff_files(input: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = None;
    let mut line_start = 0;

    while line_start < input.len() {
        let line_end = input[line_start..].find('\n').map_or(input.len(), |idx| line_start + idx + 1);
        let line = &input[line_start..line_end];

        if line.starts_with("diff --git ") {
            if let Some(s) = start {
                chunks.push(&input[s..line_start]);
            }
            start = Some(line_start);
        }

        line_start = line_end;
    }

    if let Some(s) = start {
        chunks.push(&input[s..]);
    }

    chunks
}

fn parse_file_diff(chunk: &str) -> Result<FileDiff, GitDiffError> {
    let lines: Vec<&str> = chunk.lines().collect();
    if lines.is_empty() {
        return Err(GitDiffError::ParseError("Empty diff chunk".to_string()));
    }

    let (old_path, new_path) = parse_diff_header(lines[0])?;
    let (status, binary, rename_from, hunk_start) = scan_file_metadata(&lines);
    let hunks = if binary { Vec::new() } else { parse_file_hunks(&lines[hunk_start..])? };

    Ok(FileDiff {
        old_path: resolve_old_path(status, rename_from, old_path),
        path: new_path,
        status,
        staged: StageState::Unstaged,
        hunks,
        binary,
    })
}

fn scan_file_metadata(lines: &[&str]) -> (FileStatus, bool, Option<String>, usize) {
    let mut status = FileStatus::Modified;
    let mut binary = false;
    let mut rename_from = None;
    let mut i = 1;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("new file mode") {
            status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            status = FileStatus::Deleted;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            status = FileStatus::Renamed;
            rename_from = Some(from.to_string());
        } else if line.starts_with("rename to ") {
            status = FileStatus::Renamed;
        } else if line.starts_with("Binary files ") {
            binary = true;
        } else if line.starts_with("@@") {
            break;
        }
        i += 1;
    }

    (status, binary, rename_from, i)
}

fn parse_file_hunks(lines: &[&str]) -> Result<Vec<Hunk>, GitDiffError> {
    let mut hunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if lines[i].starts_with("@@") {
            let (hunk, consumed) = parse_hunk(&lines[i..])?;
            hunks.push(hunk);
            i += consumed;
        } else {
            i += 1;
        }
    }

    Ok(hunks)
}

fn resolve_old_path(status: FileStatus, rename_from: Option<String>, old_path: String) -> Option<String> {
    if status == FileStatus::Added || status == FileStatus::Untracked {
        None
    } else if status == FileStatus::Renamed {
        rename_from.or(Some(old_path))
    } else {
        Some(old_path)
    }
}

fn parse_diff_header(line: &str) -> Result<(String, String), GitDiffError> {
    let rest = line
        .strip_prefix("diff --git ")
        .ok_or_else(|| GitDiffError::ParseError(format!("Invalid diff header: {line}")))?;

    if let Some((a, b)) = rest.split_once(" b/") {
        let old = a.strip_prefix("a/").unwrap_or(a).to_string();
        let new = b.to_string();
        Ok((old, new))
    } else {
        Err(GitDiffError::ParseError(format!("Cannot parse paths from: {line}")))
    }
}

fn parse_hunk(lines: &[&str]) -> Result<(Hunk, usize), GitDiffError> {
    let header = lines[0];
    let (old_start, old_count, new_start, new_count) = parse_hunk_header(header)?;

    let mut patch_lines = Vec::new();
    patch_lines.push(PatchLine::hunk_header(header));

    let mut old_line = old_start;
    let mut new_line = new_start;
    let mut i = 1;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@") {
            break;
        }

        if let Some(text) = line.strip_prefix('+') {
            patch_lines.push(PatchLine::added(text, new_line));
            new_line += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            patch_lines.push(PatchLine::removed(text, old_line));
            old_line += 1;
        } else if let Some(text) = line.strip_prefix(' ') {
            patch_lines.push(PatchLine::context(text, old_line, new_line));
            old_line += 1;
            new_line += 1;
        } else if line.starts_with('\\') {
            patch_lines.push(PatchLine::meta(line));
        } else {
            // Treat as context (git sometimes omits the leading space for empty lines)
            patch_lines.push(PatchLine::context(line, old_line, new_line));
            old_line += 1;
            new_line += 1;
        }
        i += 1;
    }

    Ok((Hunk { header: header.to_string(), old_start, old_count, new_start, new_count, lines: patch_lines }, i))
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize, usize, usize), GitDiffError> {
    // Format: @@ -old_start,old_count +new_start,new_count @@
    let err = || GitDiffError::ParseError(format!("Invalid hunk header: {header}"));

    let rest = header.strip_prefix("@@ -").ok_or_else(err)?;
    let at_end = rest.find(" @@").ok_or_else(err)?;
    let range_part = &rest[..at_end];

    let (old_range, new_range) = range_part.split_once(" +").ok_or_else(err)?;

    let (old_start, old_count) = parse_range(old_range).ok_or_else(err)?;
    let (new_start, new_count) = parse_range(new_range).ok_or_else(err)?;

    Ok((old_start, old_count, new_start, new_count))
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    if let Some((start, count)) = s.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        let start: usize = s.parse().ok()?;
        Some((start, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_modified_file() {
        let input = "\
diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"hello\");
     let x = 1;
 }
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[0].additions(), 1);
        assert_eq!(files[0].deletions(), 0);
        assert!(!files[0].binary);
        assert_eq!(files[0].hunks.len(), 1);
    }

    #[test]
    fn parse_added_file() {
        let input = "\
diff --git a/new_file.txt b/new_file.txt
new file mode 100644
index 0000000..abc1234
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,2 @@
+line one
+line two
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new_file.txt");
        assert_eq!(files[0].status, FileStatus::Added);
        assert!(files[0].old_path.is_none());
        assert_eq!(files[0].additions(), 2);
        assert_eq!(files[0].deletions(), 0);
    }

    #[test]
    fn parse_deleted_file() {
        let input = "\
diff --git a/old_file.txt b/old_file.txt
deleted file mode 100644
index abc1234..0000000
--- a/old_file.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-line one
-line two
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "old_file.txt");
        assert_eq!(files[0].status, FileStatus::Deleted);
        assert_eq!(files[0].additions(), 0);
        assert_eq!(files[0].deletions(), 2);
    }

    #[test]
    fn parse_renamed_file() {
        let input = "\
diff --git a/old_name.rs b/new_name.rs
similarity index 95%
rename from old_name.rs
rename to new_name.rs
index abc1234..def5678 100644
--- a/old_name.rs
+++ b/new_name.rs
@@ -1,3 +1,3 @@
 fn main() {
-    old();
+    new();
 }
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new_name.rs");
        assert_eq!(files[0].status, FileStatus::Renamed);
        assert_eq!(files[0].old_path.as_deref(), Some("old_name.rs"));
        assert_eq!(files[0].additions(), 1);
        assert_eq!(files[0].deletions(), 1);
    }

    #[test]
    fn parse_hunk_header_tracking() {
        let input = "\
diff --git a/file.rs b/file.rs
index abc..def 100644
--- a/file.rs
+++ b/file.rs
@@ -10,4 +10,5 @@ fn context_label() {
 context
-removed
+added1
+added2
 context
";
        let files = parse_unified_diff(input).unwrap();
        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.old_start, 10);
        assert_eq!(hunk.old_count, 4);
        assert_eq!(hunk.new_start, 10);
        assert_eq!(hunk.new_count, 5);

        // Check line number tracking
        let lines = &hunk.lines;
        // HunkHeader
        assert_eq!(lines[0].kind, PatchLineKind::HunkHeader);
        // context at old=10, new=10
        assert_eq!(lines[1].kind, PatchLineKind::Context);
        assert_eq!(lines[1].old_line_no, Some(10));
        assert_eq!(lines[1].new_line_no, Some(10));
        // removed at old=11
        assert_eq!(lines[2].kind, PatchLineKind::Removed);
        assert_eq!(lines[2].old_line_no, Some(11));
        assert_eq!(lines[2].new_line_no, None);
        // added at new=11
        assert_eq!(lines[3].kind, PatchLineKind::Added);
        assert_eq!(lines[3].old_line_no, None);
        assert_eq!(lines[3].new_line_no, Some(11));
        // added at new=12
        assert_eq!(lines[4].kind, PatchLineKind::Added);
        assert_eq!(lines[4].old_line_no, None);
        assert_eq!(lines[4].new_line_no, Some(12));
        // context at old=12, new=13
        assert_eq!(lines[5].kind, PatchLineKind::Context);
        assert_eq!(lines[5].old_line_no, Some(12));
        assert_eq!(lines[5].new_line_no, Some(13));
    }

    #[test]
    fn parse_meta_line() {
        let input = "\
diff --git a/file.txt b/file.txt
index abc..def 100644
--- a/file.txt
+++ b/file.txt
@@ -1,1 +1,1 @@
-old
\\ No newline at end of file
+new
";
        let files = parse_unified_diff(input).unwrap();
        let hunk = &files[0].hunks[0];
        let meta = hunk.lines.iter().find(|l| l.kind == PatchLineKind::Meta);
        assert!(meta.is_some());
        assert!(meta.unwrap().text.contains("No newline"));
    }

    #[test]
    fn parse_binary_diff() {
        let input = "\
diff --git a/image.png b/image.png
new file mode 100644
index 0000000..abc1234
Binary files /dev/null and b/image.png differ
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].binary);
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn parse_empty_diff() {
        let files = parse_unified_diff("").unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn parse_multiple_files() {
        let input = "\
diff --git a/a.rs b/a.rs
index abc..def 100644
--- a/a.rs
+++ b/a.rs
@@ -1,1 +1,1 @@
-old_a
+new_a
diff --git a/b.rs b/b.rs
new file mode 100644
index 0000000..abc1234
--- /dev/null
+++ b/b.rs
@@ -0,0 +1,1 @@
+new_b
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[1].path, "b.rs");
        assert_eq!(files[1].status, FileStatus::Added);
    }

    #[test]
    fn parse_diff_marker_inside_hunk_line() {
        let input = "\
diff --git a/file.rs b/file.rs
index abc..def 100644
--- a/file.rs
+++ b/file.rs
@@ -1,1 +1,2 @@
 fn main() {
+cannot parse paths from: diff --git /m)
 }
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "file.rs");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[0].additions(), 1);
    }

    #[test]
    fn parse_multiple_hunks() {
        let input = "\
diff --git a/file.rs b/file.rs
index abc..def 100644
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 fn a() {
-    old_a();
+    new_a();
 }
@@ -10,3 +10,3 @@
 fn b() {
-    old_b();
+    new_b();
 }
";
        let files = parse_unified_diff(input).unwrap();
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[0].old_start, 1);
        assert_eq!(files[0].hunks[1].old_start, 10);
    }

    #[test]
    fn parse_hunk_header_without_comma() {
        let (start, count, new_start, new_count) = parse_hunk_header("@@ -1 +1 @@ fn main()").unwrap();
        assert_eq!(start, 1);
        assert_eq!(count, 1);
        assert_eq!(new_start, 1);
        assert_eq!(new_count, 1);
    }

    #[test]
    fn file_status_marker() {
        assert_eq!(FileStatus::Modified.marker(), 'M');
        assert_eq!(FileStatus::Added.marker(), 'A');
        assert_eq!(FileStatus::Deleted.marker(), 'D');
        assert_eq!(FileStatus::Renamed.marker(), 'R');
        assert_eq!(FileStatus::Untracked.marker(), '?');
    }

    #[tokio::test]
    async fn build_untracked_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");
        std::fs::write(&file_path, "line one\nline two\nline three\n").unwrap();

        let diff = build_untracked_file_diff(dir.path(), "hello.txt".to_string()).await;
        assert_eq!(diff.path, "hello.txt");
        assert!(diff.old_path.is_none());
        assert_eq!(diff.status, FileStatus::Untracked);
        assert!(!diff.binary);
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.additions(), 3);
        assert_eq!(diff.deletions(), 0);

        let hunk = &diff.hunks[0];
        assert_eq!(hunk.old_start, 0);
        assert_eq!(hunk.old_count, 0);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 3);
        assert_eq!(hunk.lines[1].new_line_no, Some(1));
        assert_eq!(hunk.lines[1].text, "line one");
    }

    #[tokio::test]
    async fn build_untracked_binary_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("image.bin");
        std::fs::write(&file_path, b"PNG\x00\x00binary data").unwrap();

        let diff = build_untracked_file_diff(dir.path(), "image.bin".to_string()).await;
        assert_eq!(diff.status, FileStatus::Untracked);
        assert!(diff.binary);
        assert!(diff.hunks.is_empty());
    }

    #[tokio::test]
    async fn build_untracked_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let diff = build_untracked_file_diff(dir.path(), "does_not_exist.txt".to_string()).await;
        assert!(diff.binary);
        assert!(diff.hunks.is_empty());
    }

    async fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "--quiet"]).await.unwrap();
        git(path, &["config", "user.email", "test@example.com"]).await.unwrap();
        git(path, &["config", "user.name", "Test"]).await.unwrap();
        git(path, &["config", "commit.gpgsign", "false"]).await.unwrap();
        dir
    }

    async fn write_file(dir: &Path, name: &str, content: &str) {
        tokio::fs::write(dir.join(name), content).await.unwrap();
    }

    async fn commit_file(dir: &Path, name: &str, content: &str, message: &str) {
        write_file(dir, name, content).await;
        git(dir, &["add", "-A"]).await.unwrap();
        git(dir, &["commit", "--quiet", "-m", message]).await.unwrap();
    }

    fn staged_of(doc: &GitDiffDocument, path: &str) -> StageState {
        doc.files.iter().find(|file| file.path == path).unwrap_or_else(|| panic!("{path} not in diff")).staged
    }

    #[tokio::test]
    async fn stage_toggles_status() {
        let repo = init_repo().await;
        let dir = repo.path();
        commit_file(dir, "a.txt", "one\n", "init").await;
        write_file(dir, "a.txt", "one\ntwo\n").await;

        let doc = load_git_diff(dir, None, DiffScope::Both).await.unwrap();
        assert_eq!(staged_of(&doc, "a.txt"), StageState::Unstaged);

        stage_files(dir, &["a.txt"]).await.unwrap();
        let doc = load_git_diff(dir, None, DiffScope::Both).await.unwrap();
        assert_eq!(staged_of(&doc, "a.txt"), StageState::Staged);

        unstage_files(dir, &["a.txt"]).await.unwrap();
        let doc = load_git_diff(dir, None, DiffScope::Both).await.unwrap();
        assert_eq!(staged_of(&doc, "a.txt"), StageState::Unstaged);
    }

    #[tokio::test]
    async fn stage_all_marks_all_staged() {
        let repo = init_repo().await;
        let dir = repo.path();
        commit_file(dir, "a.txt", "a1\n", "init").await;
        commit_file(dir, "b.txt", "b1\n", "add b").await;
        write_file(dir, "a.txt", "a2\n").await;
        write_file(dir, "b.txt", "b2\n").await;
        write_file(dir, "c.txt", "c1\n").await;

        stage_all(dir).await.unwrap();
        let doc = load_git_diff(dir, None, DiffScope::Both).await.unwrap();
        assert_eq!(staged_of(&doc, "a.txt"), StageState::Staged);
        assert_eq!(staged_of(&doc, "b.txt"), StageState::Staged);
        assert_eq!(staged_of(&doc, "c.txt"), StageState::Staged);
    }

    #[tokio::test]
    async fn commit_creates_commit_and_clears_staged() {
        let repo = init_repo().await;
        let dir = repo.path();
        commit_file(dir, "a.txt", "one\n", "init").await;
        write_file(dir, "a.txt", "one\ntwo\n").await;
        stage_files(dir, &["a.txt"]).await.unwrap();
        commit(dir, "second commit").await.unwrap();
        let log = git(dir, &["log", "--oneline", "-1"]).await.unwrap();
        assert!(log.contains("second commit"), "log was: {log}");

        let doc = load_git_diff(dir, None, DiffScope::Both).await.unwrap();
        assert!(doc.files.is_empty(), "expected clean tree, got {} files", doc.files.len());
    }

    #[tokio::test]
    async fn discard_reverts_tracked_file() {
        let repo = init_repo().await;
        let dir = repo.path();
        commit_file(dir, "a.txt", "v1\n", "init").await;
        write_file(dir, "a.txt", "v2\n").await;
        discard_file(dir, "a.txt", FileStatus::Modified).await.unwrap();
        let content = tokio::fs::read_to_string(dir.join("a.txt")).await.unwrap();
        assert_eq!(content, "v1\n");
    }

    #[tokio::test]
    async fn discard_removes_untracked_file() {
        let repo = init_repo().await;
        let dir = repo.path();
        commit_file(dir, "a.txt", "a1\n", "init").await;
        write_file(dir, "new.txt", "scratch\n").await;
        discard_file(dir, "new.txt", FileStatus::Untracked).await.unwrap();
        assert!(!dir.join("new.txt").exists());
    }

    #[tokio::test]
    async fn commit_with_empty_message_is_rejected() {
        let repo = init_repo().await;
        assert!(commit(repo.path(), "   ").await.is_err());
    }

    #[tokio::test]
    async fn unstage_on_unborn_branch_untracks_file() {
        let repo = init_repo().await;
        let dir = repo.path();
        write_file(dir, "a.txt", "hi\n").await;
        git(dir, &["add", "a.txt"]).await.unwrap();
        unstage_files(dir, &["a.txt"]).await.unwrap();
        let status = git(dir, &["status", "--porcelain"]).await.unwrap();
        assert!(status.contains("?? a.txt"), "status was: {status}");
    }

    #[tokio::test]
    async fn load_git_diff_filters_changes_by_scope() {
        let repo = init_repo().await;
        let dir = repo.path();
        commit_file(dir, "a.txt", "one\n", "init").await;

        write_file(dir, "a.txt", "two\n").await;
        stage_files(dir, &["a.txt"]).await.unwrap();
        write_file(dir, "a.txt", "three\n").await;
        write_file(dir, "untracked.txt", "scratch\n").await;

        let unstaged = load_git_diff(dir, None, DiffScope::Unstaged).await.unwrap();
        assert_eq!(
            unstaged.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            vec!["a.txt", "untracked.txt"]
        );
        assert!(
            unstaged
                .files
                .iter()
                .any(|file| file.path == "a.txt" && file.hunks[0].lines.iter().any(|line| line.text == "three"))
        );

        let staged = load_git_diff(dir, None, DiffScope::Staged).await.unwrap();
        assert_eq!(staged.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), vec!["a.txt"]);
        assert!(staged.files[0].hunks[0].lines.iter().any(|line| line.text == "two"));

        let both = load_git_diff(dir, None, DiffScope::Both).await.unwrap();
        assert_eq!(
            both.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            vec!["a.txt", "untracked.txt"]
        );
        assert!(
            both.files
                .iter()
                .any(|file| file.path == "a.txt" && file.hunks[0].lines.iter().any(|line| line.text == "three"))
        );
    }

    #[test]
    fn parse_porcelain_status_handles_rename() {
        let map = parse_porcelain_status("R  new.txt\0old.txt\0 M other.txt\0");
        assert_eq!(map.get("new.txt"), Some(&StageState::Staged));
        assert_eq!(map.get("old.txt"), None);
        assert_eq!(map.get("other.txt"), Some(&StageState::Unstaged));
    }

    #[test]
    fn parse_porcelain_status_classifies_states() {
        let map = parse_porcelain_status("M  staged.txt\0 M unstaged.txt\0MM partial.txt\0?? untracked.txt\0");
        assert_eq!(map.get("staged.txt"), Some(&StageState::Staged));
        assert_eq!(map.get("unstaged.txt"), Some(&StageState::Unstaged));
        assert_eq!(map.get("partial.txt"), Some(&StageState::PartiallyStaged));
        assert_eq!(map.get("untracked.txt"), Some(&StageState::Unstaged));
    }
}
