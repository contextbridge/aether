use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Output;
use thiserror::Error;

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DiffScope {
    Both,
    Unstaged,
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StageState {
    Unstaged,
    Staged,
    PartiallyStaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub stage_state: StageState,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GitSnapshot {
    pub id: String,
    pub repo_root: String,
    pub patch: String,
    pub files: Vec<GitFile>,
    pub scope: DiffScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiffFileContents {
    pub old_contents: Option<String>,
    pub new_contents: Option<String>,
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("Not a git repository")]
    NotARepository,
    #[error("Git command failed: {0}")]
    CommandFailed(String),
    #[error("Invalid repository path: {0}")]
    InvalidPath(String),
    #[error("Cannot read {path}: {message}")]
    ReadFailed { path: String, message: String },
}

#[derive(Debug, Clone)]
pub struct GitRepository {
    working_dir: PathBuf,
}

impl GitRepository {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    pub async fn snapshot(&self, scope: DiffScope) -> Result<GitSnapshot, GitError> {
        let root = self.repo_root().await?;
        let status_output = run_git(&root, &["status", "--porcelain=v1", "-z"]).await?;
        let statuses = parse_status(&status_output.stdout);
        let args = diff_args(&root, scope).await?;
        let patch_output = run_git(&root, &args).await?;
        let mut patch = String::from_utf8_lossy(&patch_output.stdout).into_owned();
        let names = diff_names(&root, &args).await?;
        let stats = diff_stats(&root, &args).await?;
        let mut files = names
            .into_iter()
            .map(|(status, old_path, path)| {
                let stage_state = statuses.get(&path).map_or(StageState::Unstaged, |entry| entry.stage_state);
                let (additions, deletions, binary) = stats.get(&path).copied().unwrap_or((Some(0), Some(0), false));
                GitFile { path, old_path, status, stage_state, additions, deletions, binary }
            })
            .collect::<Vec<_>>();

        if scope != DiffScope::Staged {
            for path in untracked_paths(&root).await? {
                let bytes = read_worktree_bytes(&root, &path).await?;
                let binary = bytes.contains(&0) || std::str::from_utf8(&bytes).is_err();
                let additions = (!binary).then(|| line_count(&bytes));
                if !binary {
                    append_untracked_patch(&mut patch, &path, &String::from_utf8_lossy(&bytes));
                }
                files.push(GitFile {
                    path,
                    old_path: None,
                    status: FileStatus::Untracked,
                    stage_state: StageState::Unstaged,
                    additions,
                    deletions: additions.map(|_| 0),
                    binary,
                });
            }
        }

        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(GitSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            repo_root: root.to_string_lossy().into_owned(),
            patch,
            files,
            scope,
        })
    }

    pub async fn stage(&self, paths: &[String]) -> Result<(), GitError> {
        let root = self.repo_root().await?;
        let paths = validate_paths(&root, paths)?;
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        run_git(&root, &args).await.map(drop)
    }

    pub async fn unstage(&self, paths: &[String]) -> Result<(), GitError> {
        let root = self.repo_root().await?;
        let paths = validate_paths(&root, paths)?;
        let mut args = vec!["reset", "--quiet", "--"];
        args.extend(paths.iter().map(String::as_str));
        run_git(&root, &args).await.map(drop)
    }

    pub async fn stage_all(&self) -> Result<(), GitError> {
        let root = self.repo_root().await?;
        run_git(&root, &["add", "-A"]).await.map(drop)
    }

    pub async fn unstage_all(&self) -> Result<(), GitError> {
        let root = self.repo_root().await?;
        run_git(&root, &["reset", "--quiet"]).await.map(drop)
    }

    pub async fn commit(&self, message: &str) -> Result<(), GitError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(GitError::CommandFailed("empty commit message".to_string()));
        }
        let root = self.repo_root().await?;
        run_git(&root, &["commit", "-m", message]).await.map(drop)
    }

    pub async fn discard(&self, path: &str, old_path: Option<&str>, status: FileStatus) -> Result<(), GitError> {
        let root = self.repo_root().await?;
        let paths = validate_paths(&root, &[path.to_string()])?;
        let path = paths[0].as_str();
        match status {
            FileStatus::Untracked => run_git(&root, &["clean", "-f", "--", path]).await.map(drop),
            FileStatus::Added => run_git(&root, &["rm", "-f", "--", path]).await.map(drop),
            FileStatus::Renamed => {
                let old_path = old_path.ok_or_else(|| GitError::InvalidPath(path.to_string()))?;
                validate_paths(&root, &[old_path.to_string()])?;
                run_git(&root, &["reset", "--quiet", "HEAD", "--", old_path, path]).await?;
                run_git(&root, &["checkout", "--", old_path]).await?;
                run_git(&root, &["clean", "-f", "--", path]).await.map(drop)
            }
            FileStatus::Modified | FileStatus::Deleted => {
                run_git(&root, &["restore", "--source=HEAD", "--staged", "--worktree", "--", path]).await.map(drop)
            }
        }
    }

    pub async fn load_file_contents(
        &self,
        path: &str,
        old_path: Option<&str>,
        scope: DiffScope,
    ) -> Result<DiffFileContents, GitError> {
        let root = self.repo_root().await?;
        let paths = validate_paths(&root, &[path.to_string()])?;
        let path = paths[0].as_str();
        let old_path = old_path.unwrap_or(path);
        validate_paths(&root, &[old_path.to_string()])?;
        let old_contents = match scope {
            DiffScope::Unstaged => git_text_optional(&root, &["show", &format!(":{old_path}")]).await?,
            DiffScope::Both | DiffScope::Staged => {
                git_text_optional(&root, &["show", &format!("HEAD:{old_path}")]).await?
            }
        };
        let new_contents = match scope {
            DiffScope::Staged => git_text_optional(&root, &["show", &format!(":{path}")]).await?,
            DiffScope::Both | DiffScope::Unstaged => read_worktree_text_optional(&root, path).await?,
        };
        Ok(DiffFileContents { old_contents, new_contents })
    }

    async fn repo_root(&self) -> Result<PathBuf, GitError> {
        let output = run_git(&self.working_dir, &["rev-parse", "--show-toplevel"])
            .await
            .map_err(|_| GitError::NotARepository)?;
        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
            .canonicalize()
            .map_err(|error| GitError::CommandFailed(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusEntry {
    status: FileStatus,
    stage_state: StageState,
}

async fn read_worktree_bytes(root: &Path, file_path: &str) -> Result<Vec<u8>, GitError> {
    validate_paths(root, &[file_path.to_string()])?;
    let path = root.join(file_path);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|error| GitError::ReadFailed { path: file_path.to_string(), message: error.to_string() })?;
    if metadata.file_type().is_symlink() {
        return tokio::fs::read_link(path)
            .await
            .map(|target| target.to_string_lossy().into_owned().into_bytes())
            .map_err(|error| GitError::ReadFailed { path: file_path.to_string(), message: error.to_string() });
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| GitError::ReadFailed { path: file_path.to_string(), message: error.to_string() })?;
    if !canonical.starts_with(root) {
        return Err(GitError::InvalidPath(file_path.to_string()));
    }
    tokio::fs::read(canonical)
        .await
        .map_err(|error| GitError::ReadFailed { path: file_path.to_string(), message: error.to_string() })
}

async fn read_worktree_text_optional(root: &Path, file_path: &str) -> Result<Option<String>, GitError> {
    match read_worktree_bytes(root, file_path).await {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
        Err(GitError::ReadFailed { .. }) if !root.join(file_path).exists() => Ok(None),
        Err(error) => Err(error),
    }
}

async fn run_git(root: &Path, args: &[&str]) -> Result<Output, GitError> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|error| GitError::CommandFailed(error.to_string()))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(GitError::CommandFailed(String::from_utf8_lossy(&output.stderr).trim().to_string()))
    }
}

async fn git_text_optional(root: &Path, args: &[&str]) -> Result<Option<String>, GitError> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|error| GitError::CommandFailed(error.to_string()))?;
    if output.status.success() { Ok(String::from_utf8(output.stdout).ok()) } else { Ok(None) }
}

async fn diff_args(root: &Path, scope: DiffScope) -> Result<Vec<&'static str>, GitError> {
    let mut args = match scope {
        DiffScope::Staged => vec!["diff", "--cached", "--no-ext-diff", "--find-renames", "--binary"],
        DiffScope::Both | DiffScope::Unstaged => vec!["diff", "--no-ext-diff", "--find-renames", "--binary"],
    };
    if scope == DiffScope::Both {
        let has_head = run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await.is_ok();
        args.push(if has_head { "HEAD" } else { EMPTY_TREE });
    }
    Ok(args)
}

async fn untracked_paths(root: &Path) -> Result<Vec<String>, GitError> {
    let output = run_git(root, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    Ok(output.stdout.split(|byte| *byte == 0).filter(|field| !field.is_empty()).map(text).collect())
}

async fn diff_names(root: &Path, diff_args: &[&str]) -> Result<Vec<(FileStatus, Option<String>, String)>, GitError> {
    let mut args = diff_args.to_vec();
    args.retain(|arg| *arg != "--binary");
    args.push("--name-status");
    args.push("-z");
    let output = run_git(root, &args).await?;
    let mut fields = output.stdout.split(|byte| *byte == 0).filter(|field| !field.is_empty());
    let mut files = Vec::new();
    while let Some(raw_status) = fields.next() {
        let status_code = raw_status[0] as char;
        let Some(first_path) = fields.next() else { break };
        if status_code == 'R' {
            let Some(new_path) = fields.next() else { break };
            files.push((FileStatus::Renamed, Some(text(first_path)), text(new_path)));
        } else {
            files.push((status_from_code(status_code), None, text(first_path)));
        }
    }
    Ok(files)
}

async fn diff_stats(
    root: &Path,
    diff_args: &[&str],
) -> Result<HashMap<String, (Option<u32>, Option<u32>, bool)>, GitError> {
    let mut args = diff_args.to_vec();
    args.retain(|arg| *arg != "--binary");
    args.splice(0..0, ["-c", "core.quotepath=false"]);
    args.push("--numstat");
    let output = run_git(root, &args).await?;
    let mut result = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(additions), Some(deletions), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let binary = additions == "-" || deletions == "-";
        let path = path.rsplit(" => ").next().unwrap_or(path).trim_matches(['{', '}']).to_string();
        result.insert(path, (additions.parse().ok(), deletions.parse().ok(), binary));
    }
    Ok(result)
}

fn parse_status(output: &[u8]) -> HashMap<String, StatusEntry> {
    let fields = output.split(|byte| *byte == 0).filter(|field| !field.is_empty()).collect::<Vec<_>>();
    let mut result = HashMap::new();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        let x = field[0] as char;
        let y = field[1] as char;
        let path = text(&field[3..]);
        let status =
            if x == '?' && y == '?' { FileStatus::Untracked } else { status_from_code(if y == ' ' { x } else { y }) };
        let stage_state = if x != ' ' && x != '?' && y != ' ' {
            StageState::PartiallyStaged
        } else if x != ' ' && x != '?' {
            StageState::Staged
        } else {
            StageState::Unstaged
        };
        if x == 'R' || y == 'R' {
            index += 1;
        }
        result.insert(path, StatusEntry { status, stage_state });
        index += 1;
    }
    result
}

fn status_from_code(code: char) -> FileStatus {
    match code {
        'A' => FileStatus::Added,
        'D' => FileStatus::Deleted,
        'R' => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

fn validate_paths(root: &Path, paths: &[String]) -> Result<Vec<String>, GitError> {
    paths
        .iter()
        .map(|path| {
            let candidate = Path::new(path);
            if candidate.is_absolute()
                || candidate.components().any(|component| {
                    matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
                })
            {
                return Err(GitError::InvalidPath(path.clone()));
            }
            let parent = root
                .join(candidate)
                .parent()
                .unwrap_or(root)
                .canonicalize()
                .map_err(|_| GitError::InvalidPath(path.clone()))?;
            if !parent.starts_with(root) {
                return Err(GitError::InvalidPath(path.clone()));
            }
            Ok(path.clone())
        })
        .collect()
}

fn append_untracked_patch(output: &mut String, file_path: &str, contents: &str) {
    use std::fmt::Write;
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    let lines = contents.lines().count();
    let _ = writeln!(output, "diff --git a/{file_path} b/{file_path}");
    let _ = writeln!(output, "new file mode 100644");
    let _ = writeln!(output, "--- /dev/null");
    let _ = writeln!(output, "+++ b/{file_path}");
    let _ = writeln!(output, "@@ -0,0 +1,{lines} @@");
    for line in contents.lines() {
        let _ = writeln!(output, "+{line}");
    }
}

fn line_count(bytes: &[u8]) -> u32 {
    let lines = std::str::from_utf8(bytes).map_or(0, |contents| contents.lines().count());
    u32::try_from(lines).unwrap_or(u32::MAX)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
