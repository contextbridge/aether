use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffDocument {
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

impl PatchLine {
    pub fn added(text: impl Into<String>, new_line_no: usize) -> Self {
        Self { kind: PatchLineKind::Added, text: text.into(), old_line_no: None, new_line_no: Some(new_line_no) }
    }

    pub fn removed(text: impl Into<String>, old_line_no: usize) -> Self {
        Self { kind: PatchLineKind::Removed, text: text.into(), old_line_no: Some(old_line_no), new_line_no: None }
    }

    pub fn context(text: impl Into<String>, old_line_no: usize, new_line_no: usize) -> Self {
        Self {
            kind: PatchLineKind::Context,
            text: text.into(),
            old_line_no: Some(old_line_no),
            new_line_no: Some(new_line_no),
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PatchAnchor {
    pub file_index: usize,
    pub hunk: usize,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct CommentContext {
    pub file_path: String,
    pub line_text: String,
    pub line_number: Option<usize>,
    pub line_kind: PatchLineKind,
}

#[derive(Debug, Clone)]
pub struct QueuedComment {
    pub anchor: PatchAnchor,
    pub body: String,
    pub context: CommentContext,
}

#[derive(Debug, Clone, Default)]
pub struct ReviewQueue {
    comments: Vec<QueuedComment>,
}

impl ReviewQueue {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    pub fn len(&self) -> usize {
        self.comments.len()
    }

    pub fn clear(&mut self) {
        self.comments.clear();
    }

    pub fn push(&mut self, comment: QueuedComment) {
        self.comments.push(comment);
    }

    pub fn pop(&mut self) -> Option<QueuedComment> {
        self.comments.pop()
    }

    pub fn comments_for_file(&self, file_path: &str) -> impl Iterator<Item = &QueuedComment> {
        self.comments.iter().filter(move |c| c.context.file_path == file_path)
    }

    pub fn comments(&self) -> &[QueuedComment] {
        &self.comments
    }

    pub fn format_prompt(&self) -> String {
        let mut prompt = String::from("I'm reviewing the working tree diff. Here are my comments:\n");
        let mut file_order: Vec<&str> = Vec::new();
        let mut grouped: HashMap<&str, Vec<&QueuedComment>> = HashMap::new();

        for comment in &self.comments {
            let path = comment.context.file_path.as_str();
            if !grouped.contains_key(path) {
                file_order.push(path);
            }
            grouped.entry(path).or_default().push(comment);
        }

        for file_path in file_order {
            let file_comments = grouped.get(file_path).expect("group exists for ordered path");
            write!(prompt, "\n## `{file_path}`\n").unwrap();

            for comment in file_comments {
                let kind_label = match comment.context.line_kind {
                    PatchLineKind::Added => "added",
                    PatchLineKind::Removed => "removed",
                    PatchLineKind::Context => "context",
                    PatchLineKind::HunkHeader => "header",
                    PatchLineKind::Meta => "meta",
                };
                let line_ref = match comment.context.line_number {
                    Some(n) => format!("Line {n} ({kind_label})"),
                    None => kind_label.to_string(),
                };
                write!(prompt, "\n**{line_ref}:** `{}`\n> {}\n", comment.context.line_text, comment.body).unwrap();
            }
        }

        prompt
    }
}

impl DiffDocument {
    /// Normalizes command output into the model consumed by both review renderers.
    ///
    /// Git execution deliberately happens outside this type. Keeping this operation
    /// synchronous makes parsing deterministic and lets tests exercise it without a
    /// repository or subprocess.
    pub fn from_git_output(
        repo_root: PathBuf,
        diff_output: &str,
        status_output: &str,
        untracked_files: impl IntoIterator<Item = (String, Vec<u8>)>,
        scope: DiffScope,
    ) -> Result<Self, GitDiffError> {
        let mut files = if diff_output.trim().is_empty() { Vec::new() } else { parse_unified_diff(diff_output)? };

        if scope.includes_untracked() {
            files.extend(untracked_files.into_iter().map(|(path, bytes)| build_untracked_file_diff(path, &bytes)));
        }

        let status_map = parse_porcelain_status(status_output);
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
    /// Builds the canonical model used for ACP tool previews and Git reviews.
    pub fn from_texts(path: impl Into<String>, old: &str, new: &str) -> Self {
        let path = path.into();
        let status = match (old.is_empty(), new.is_empty()) {
            (true, false) => FileStatus::Added,
            (false, true) => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };

        let diff = similar::TextDiff::from_lines(old, new);
        let formatter = diff.unified_diff();
        let hunks = formatter.iter_hunks().map(|hunk| unified_hunk(&hunk)).collect();

        Self {
            old_path: (status != FileStatus::Added).then(|| path.clone()),
            path,
            status,
            staged: StageState::Unstaged,
            hunks,
            binary: false,
        }
    }

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

pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub fn parse_porcelain_status(input: &str) -> HashMap<String, StageState> {
    let mut map = HashMap::new();
    let mut tokens = input.split('\0').filter(|token| !token.is_empty());

    while let Some(record) = tokens.next() {
        if record.len() < 3 {
            continue;
        }
        let bytes = record.as_bytes();
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        let path = if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            tokens.next().unwrap_or(&record[3..])
        } else {
            &record[3..]
        };

        let state = match (index, worktree) {
            ('?', '?') | (' ', _) => StageState::Unstaged,
            (_, ' ') => StageState::Staged,
            _ => StageState::PartiallyStaged,
        };
        map.insert(path.to_string(), state);
    }

    map
}

pub(crate) fn build_untracked_file_diff(path: String, bytes: &[u8]) -> FileDiff {
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return binary_untracked(path);
    }
    let Ok(content) = std::str::from_utf8(bytes) else {
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

pub(crate) fn binary_untracked(path: String) -> FileDiff {
    FileDiff {
        old_path: None,
        path,
        status: FileStatus::Untracked,
        staged: StageState::Unstaged,
        hunks: Vec::new(),
        binary: true,
    }
}

pub fn parse_unified_diff(input: &str) -> Result<Vec<FileDiff>, GitDiffError> {
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

fn unified_hunk(hunk: &similar::udiff::UnifiedDiffHunk<'_, '_, '_, str>) -> Hunk {
    let ops = hunk.ops();
    let old = unified_range(ops[0].old_range().start, ops[ops.len() - 1].old_range().end);
    let new = unified_range(ops[0].new_range().start, ops[ops.len() - 1].new_range().end);
    let header = hunk.header().to_string();
    let mut lines = vec![PatchLine {
        kind: PatchLineKind::HunkHeader,
        text: header.clone(),
        old_line_no: None,
        new_line_no: None,
    }];
    lines.extend(hunk.iter_changes().map(unified_change));

    Hunk { header, old_start: old.0, old_count: old.1, new_start: new.0, new_count: new.1, lines }
}

fn unified_change(change: similar::Change<&str>) -> PatchLine {
    let text = || line_without_terminator(change.value()).to_string();
    match change.tag() {
        similar::ChangeTag::Equal => {
            PatchLine::context(text(), change.old_index().unwrap_or(0) + 1, change.new_index().unwrap_or(0) + 1)
        }
        similar::ChangeTag::Delete => PatchLine::removed(text(), change.old_index().unwrap_or(0) + 1),
        similar::ChangeTag::Insert => PatchLine::added(text(), change.new_index().unwrap_or(0) + 1),
    }
}

/// Converts similar's 0-based op range into the 1-based `(start, count)` pair a unified
/// diff header encodes, so `Hunk` stays consistent with its own header.
fn unified_range(start: usize, end: usize) -> (usize, usize) {
    let count = end - start;
    let start = if count == 0 { start } else { start + 1 };
    (start, count)
}

fn line_without_terminator(line: &str) -> &str {
    let without_lf = line.strip_suffix('\n').unwrap_or(line);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
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
            let result = PatchLine::added(text, new_line);
            new_line += 1;
            result
        } else if let Some(text) = line.strip_prefix('-') {
            let result = PatchLine::removed(text, old_line);
            old_line += 1;
            result
        } else if let Some(text) = line.strip_prefix(' ') {
            let result = PatchLine::context(text, old_line, new_line);
            old_line += 1;
            new_line += 1;
            result
        } else if line.starts_with('\\') {
            PatchLine { kind: PatchLineKind::Meta, text: line.to_string(), old_line_no: None, new_line_no: None }
        } else {
            let result = PatchLine::context(line, old_line, new_line);
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
