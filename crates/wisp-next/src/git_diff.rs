use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDiffDocument {
    pub repo_root: PathBuf,
    pub files: Vec<FileDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub path: String,
    pub status: FileStatus,
    pub staged: StageState,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<PatchLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLine {
    pub kind: PatchLineKind,
    pub text: String,
    pub old_line_no: Option<usize>,
    pub new_line_no: Option<usize>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchLineKind {
    HunkHeader,
    Context,
    Added,
    Removed,
    Meta,
}

#[derive(Debug, Error)]
pub enum GitDiffError {
    #[error("Not a git repository")]
    NotARepository,
    #[error("Git command failed: {stderr}")]
    CommandFailed { stderr: String },
    #[error("Failed to parse diff: {0}")]
    ParseError(String),
}

impl GitDiffDocument {
    pub async fn load(
        working_dir: &Path,
        cached_repo_root: Option<&Path>,
        scope: DiffScope,
    ) -> Result<Self, GitDiffError> {
        let repo_root = match cached_repo_root {
            Some(root) => root.to_path_buf(),
            None => resolve_repo_root(working_dir).await?,
        };
        let diff_args = match scope {
            DiffScope::Unstaged => &["diff", "--no-ext-diff", "--find-renames"][..],
            DiffScope::Staged => &["diff", "--cached", "--no-ext-diff", "--find-renames"][..],
            DiffScope::Both => &["diff", "--no-ext-diff", "--find-renames", "HEAD"][..],
        };
        let diff_output = match git(&repo_root, diff_args).await {
            Ok(output) => output,
            Err(GitDiffError::CommandFailed { stderr }) if scope == DiffScope::Both && stderr.contains("HEAD") => {
                git(&repo_root, &["diff", "--no-ext-diff", "--find-renames", EMPTY_TREE]).await?
            }
            Err(error) => return Err(error),
        };
        let mut files = if diff_output.trim().is_empty() { Vec::new() } else { parse_unified_diff(&diff_output)? };

        if scope.includes_untracked() {
            let untracked = git(&repo_root, &["ls-files", "--others", "--exclude-standard"]).await?;
            for path in untracked.lines().filter(|line| !line.is_empty()) {
                files.push(build_untracked_file_diff(&repo_root, path.to_string()).await);
            }
        }

        let status_map = load_status_map(&repo_root).await?;
        for file in &mut files {
            file.staged = status_map.get(&file.path).copied().unwrap_or(StageState::Unstaged);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));

        Ok(Self { repo_root, files })
    }
}

impl DiffScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
            Self::Both => "Both",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Both => Self::Unstaged,
            Self::Unstaged => Self::Staged,
            Self::Staged => Self::Both,
        }
    }

    pub fn includes_untracked(self) -> bool {
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

    pub fn language(&self) -> &str {
        Path::new(&self.path).extension().and_then(|extension| extension.to_str()).unwrap_or_default()
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

pub async fn stage_files(repo_root: &Path, paths: &[String]) -> Result<(), GitDiffError> {
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(repo_root, &args).await.map(drop)
}

pub async fn unstage_files(repo_root: &Path, paths: &[String]) -> Result<(), GitDiffError> {
    let mut args = vec!["reset", "--quiet", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(repo_root, &args).await.map(drop)
}

pub async fn stage_all(repo_root: &Path) -> Result<(), GitDiffError> {
    git(repo_root, &["add", "-A"]).await.map(drop)
}

pub async fn unstage_all(repo_root: &Path) -> Result<(), GitDiffError> {
    git(repo_root, &["reset", "--quiet"]).await.map(drop)
}

pub async fn commit(repo_root: &Path, message: &str) -> Result<(), GitDiffError> {
    if message.trim().is_empty() {
        return Err(GitDiffError::CommandFailed { stderr: "empty commit message".to_string() });
    }
    git(repo_root, &["commit", "-m", message]).await.map(drop)
}

pub async fn discard_file(repo_root: &Path, path: &str, status: FileStatus) -> Result<(), GitDiffError> {
    match status {
        FileStatus::Untracked => git(repo_root, &["clean", "-f", "--", path]).await.map(drop),
        _ => git(repo_root, &["restore", "--source=HEAD", "--staged", "--worktree", "--", path]).await.map(drop),
    }
}

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

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
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|error| GitDiffError::CommandFailed { stderr: error.to_string() })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Err(GitDiffError::NotARepository);
        }
        return Err(GitDiffError::CommandFailed { stderr: stderr.into_owned() });
    }

    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

async fn git(repo_root: &Path, args: &[&str]) -> Result<String, GitDiffError> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|error| GitDiffError::CommandFailed { stderr: error.to_string() })?;

    if !output.status.success() {
        return Err(GitDiffError::CommandFailed { stderr: String::from_utf8_lossy(&output.stderr).into_owned() });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn build_untracked_file_diff(repo_root: &Path, path: String) -> FileDiff {
    let Ok(bytes) = tokio::fs::read(repo_root.join(&path)).await else {
        return binary_untracked(path);
    };
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return binary_untracked(path);
    }
    let Ok(content) = String::from_utf8(bytes) else {
        return binary_untracked(path);
    };
    let source_lines: Vec<&str> = content.lines().collect();
    let line_count = source_lines.len();
    let header = format!("@@ -0,0 +1,{line_count} @@");
    let mut lines =
        vec![PatchLine { kind: PatchLineKind::HunkHeader, text: header.clone(), old_line_no: None, new_line_no: None }];
    lines.extend(source_lines.iter().enumerate().map(|(index, text)| PatchLine {
        kind: PatchLineKind::Added,
        text: (*text).to_string(),
        old_line_no: None,
        new_line_no: Some(index + 1),
    }));

    FileDiff {
        old_path: None,
        path,
        status: FileStatus::Untracked,
        staged: StageState::Unstaged,
        hunks: vec![Hunk { header, old_start: 0, old_count: 0, new_start: 1, new_count: line_count, lines }],
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

fn parse_unified_diff(input: &str) -> Result<Vec<FileDiff>, GitDiffError> {
    split_diff_files(input).into_iter().map(parse_file_diff).collect()
}

fn split_diff_files(input: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = None;
    let mut line_start = 0;

    while line_start < input.len() {
        let line_end = input[line_start..].find('\n').map_or(input.len(), |index| line_start + index + 1);
        let line = &input[line_start..line_end];
        if line.starts_with("diff --git ") {
            if let Some(chunk_start) = start {
                chunks.push(&input[chunk_start..line_start]);
            }
            start = Some(line_start);
        }
        line_start = line_end;
    }
    if let Some(chunk_start) = start {
        chunks.push(&input[chunk_start..]);
    }
    chunks
}

fn parse_file_diff(chunk: &str) -> Result<FileDiff, GitDiffError> {
    let lines: Vec<&str> = chunk.lines().collect();
    let Some(header) = lines.first() else {
        return Err(GitDiffError::ParseError("Empty diff chunk".to_string()));
    };
    let (old_path, new_path) = parse_diff_header(header)?;
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
    let mut index = 1;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("new file mode") {
            status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            status = FileStatus::Deleted;
        } else if let Some(path) = line.strip_prefix("rename from ") {
            status = FileStatus::Renamed;
            rename_from = Some(path.to_string());
        } else if line.starts_with("rename to ") {
            status = FileStatus::Renamed;
        } else if line.starts_with("Binary files ") {
            binary = true;
        } else if line.starts_with("@@") {
            break;
        }
        index += 1;
    }
    (status, binary, rename_from, index)
}

fn parse_file_hunks(lines: &[&str]) -> Result<Vec<Hunk>, GitDiffError> {
    let mut hunks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].starts_with("@@") {
            let (hunk, consumed) = parse_hunk(&lines[index..])?;
            hunks.push(hunk);
            index += consumed;
        } else {
            index += 1;
        }
    }
    Ok(hunks)
}

fn resolve_old_path(status: FileStatus, rename_from: Option<String>, old_path: String) -> Option<String> {
    match status {
        FileStatus::Added | FileStatus::Untracked => None,
        FileStatus::Renamed => rename_from.or(Some(old_path)),
        _ => Some(old_path),
    }
}

fn parse_diff_header(line: &str) -> Result<(String, String), GitDiffError> {
    let rest = line
        .strip_prefix("diff --git ")
        .ok_or_else(|| GitDiffError::ParseError(format!("Invalid diff header: {line}")))?;
    if let Some((old, new)) = rest.split_once(" b/") {
        Ok((old.strip_prefix("a/").unwrap_or(old).to_string(), new.to_string()))
    } else {
        Err(GitDiffError::ParseError(format!("Cannot parse paths from: {line}")))
    }
}

fn parse_hunk(lines: &[&str]) -> Result<(Hunk, usize), GitDiffError> {
    let header = lines[0];
    let (old_start, old_count, new_start, new_count) = parse_hunk_header(header)?;
    let mut patch_lines = vec![PatchLine {
        kind: PatchLineKind::HunkHeader,
        text: header.to_string(),
        old_line_no: None,
        new_line_no: None,
    }];
    let mut old_line = old_start;
    let mut new_line = new_start;
    let mut index = 1;

    while index < lines.len() && !lines[index].starts_with("@@") {
        let line = lines[index];
        let patch_line = if let Some(text) = line.strip_prefix('+') {
            let result = PatchLine {
                kind: PatchLineKind::Added,
                text: text.to_string(),
                old_line_no: None,
                new_line_no: Some(new_line),
            };
            new_line += 1;
            result
        } else if let Some(text) = line.strip_prefix('-') {
            let result = PatchLine {
                kind: PatchLineKind::Removed,
                text: text.to_string(),
                old_line_no: Some(old_line),
                new_line_no: None,
            };
            old_line += 1;
            result
        } else if let Some(text) = line.strip_prefix(' ') {
            let result = PatchLine {
                kind: PatchLineKind::Context,
                text: text.to_string(),
                old_line_no: Some(old_line),
                new_line_no: Some(new_line),
            };
            old_line += 1;
            new_line += 1;
            result
        } else if line.starts_with('\\') {
            PatchLine { kind: PatchLineKind::Meta, text: line.to_string(), old_line_no: None, new_line_no: None }
        } else {
            let result = PatchLine {
                kind: PatchLineKind::Context,
                text: line.to_string(),
                old_line_no: Some(old_line),
                new_line_no: Some(new_line),
            };
            old_line += 1;
            new_line += 1;
            result
        };
        patch_lines.push(patch_line);
        index += 1;
    }

    Ok((Hunk { header: header.to_string(), old_start, old_count, new_start, new_count, lines: patch_lines }, index))
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize, usize, usize), GitDiffError> {
    let invalid = || GitDiffError::ParseError(format!("Invalid hunk header: {header}"));
    let rest = header.strip_prefix("@@ -").ok_or_else(invalid)?;
    let end = rest.find(" @@").ok_or_else(invalid)?;
    let (old_range, new_range) = rest[..end].split_once(" +").ok_or_else(invalid)?;
    let (old_start, old_count) = parse_range(old_range).ok_or_else(invalid)?;
    let (new_start, new_count) = parse_range(new_range).ok_or_else(invalid)?;
    Ok((old_start, old_count, new_start, new_count))
}

fn parse_range(value: &str) -> Option<(usize, usize)> {
    if let Some((start, count)) = value.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((value.parse().ok()?, 1))
    }
}
