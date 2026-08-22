use crate::command::GitCommand;
use crate::git_review::{DiffDocument, DiffScope, EMPTY_TREE, FileStatus, GitDiffError, GitDiffEvent};
use crate::session::workspace_status::{WorkspaceStatus, home_relative_path};
use std::path::{Path, PathBuf};
use std::process::Output;

pub async fn execute(command: GitCommand) -> GitDiffEvent {
    match command {
        GitCommand::Load { request_id, working_dir, repo_root, scope } => {
            GitDiffEvent::Loaded { request_id, result: load_diff(&working_dir, repo_root.as_deref(), scope).await }
        }
        GitCommand::StageFiles { request_id, repo_root, paths } => {
            let mut args = vec!["--"];
            args.extend(paths.iter().map(String::as_str));
            GitDiffEvent::ActionFinished { request_id, result: run_action(&repo_root, "add", args).await }
        }
        GitCommand::UnstageFiles { request_id, repo_root, paths } => {
            let mut args = vec!["--quiet", "--"];
            args.extend(paths.iter().map(String::as_str));
            GitDiffEvent::ActionFinished { request_id, result: run_action(&repo_root, "reset", args).await }
        }
        GitCommand::StageAll { request_id, repo_root } => {
            GitDiffEvent::ActionFinished { request_id, result: run_action(&repo_root, "add", vec!["-A"]).await }
        }
        GitCommand::UnstageAll { request_id, repo_root } => GitDiffEvent::ActionFinished {
            request_id,
            result: run_action(&repo_root, "reset", vec!["--quiet"]).await,
        },
        GitCommand::Commit { request_id, repo_root, message } => {
            let result = if message.trim().is_empty() {
                Err(GitDiffError::CommandFailed { stderr: "empty commit message".to_string() })
            } else {
                run_action(&repo_root, "commit", vec!["-m", message.as_str()]).await
            };
            GitDiffEvent::ActionFinished { request_id, result }
        }
        GitCommand::DiscardFile { request_id, repo_root, path, status } => {
            let (command, args) = match status {
                FileStatus::Untracked => ("clean", vec!["-f", "--", path.as_str()]),
                _ => ("restore", vec!["--source=HEAD", "--staged", "--worktree", "--", path.as_str()]),
            };
            GitDiffEvent::ActionFinished { request_id, result: run_action(&repo_root, command, args).await }
        }
        GitCommand::LoadFullFile { request_id, repo_root, path } => {
            let result = tokio::fs::read_to_string(repo_root.join(&path))
                .await
                .map_err(|error| GitDiffError::CommandFailed { stderr: format!("Cannot read {path}: {error}") });
            GitDiffEvent::FullFileLoaded { request_id, path, result }
        }
    }
}

pub async fn resolve_workspace_status(cwd: &Path) -> WorkspaceStatus {
    let git_ref = match run_output(cwd, &["branch", "--show-current"]).await.ok().and_then(|output| non_empty(&output)) {
        Some(reference) => Some(reference),
        None => run_output(cwd, &["rev-parse", "--short", "HEAD"])
            .await
            .ok()
            .and_then(|output| non_empty(&output)),
    };
    WorkspaceStatus::new(home_relative_path(cwd), git_ref)
}

async fn run_output(repo_root: &Path, args: &[&str]) -> Result<Output, GitDiffError> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|error| GitDiffError::CommandFailed { stderr: error.to_string() })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(GitDiffError::CommandFailed { stderr: String::from_utf8_lossy(&output.stderr).into_owned() })
    }
}

async fn load_diff(
    working_dir: &Path,
    cached_repo_root: Option<&Path>,
    scope: DiffScope,
) -> Result<DiffDocument, GitDiffError> {
    let repo_root = match cached_repo_root {
        Some(root) => root.to_path_buf(),
        None => resolve_repo_root(working_dir).await?,
    };
    let mut diff_args = match scope {
        DiffScope::Staged => vec!["diff", "--cached", "--no-ext-diff", "--find-renames"],
        DiffScope::Unstaged | DiffScope::Both => vec!["diff", "--no-ext-diff", "--find-renames"],
    };
    if scope == DiffScope::Both {
        diff_args.push(if succeeds(&repo_root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await {
            "HEAD"
        } else {
            EMPTY_TREE
        });
    }
    let diff_output = run_output(&repo_root, &diff_args).await?;
    let status_output = run_output(&repo_root, &["status", "--porcelain=v1", "-z"]).await?;
    let mut untracked = Vec::new();
    if scope != DiffScope::Staged {
        let paths = run_output(&repo_root, &["ls-files", "--others", "--exclude-standard"]).await?;
        for path in String::from_utf8_lossy(&paths.stdout).lines().filter(|path| !path.is_empty()) {
            let bytes = tokio::fs::read(repo_root.join(path)).await.unwrap_or_default();
            untracked.push((path.to_string(), bytes));
        }
    }
    DiffDocument::from_git_output(
        repo_root,
        &String::from_utf8_lossy(&diff_output.stdout),
        &String::from_utf8_lossy(&status_output.stdout),
        untracked,
        scope,
    )
}

async fn run_action(repo_root: &Path, command: &str, args: Vec<&str>) -> Result<(), GitDiffError> {
    run_output(repo_root, &std::iter::once(command).chain(args).collect::<Vec<_>>()).await.map(drop)
}

async fn resolve_repo_root(working_dir: &Path) -> Result<PathBuf, GitDiffError> {
    match run_output(working_dir, &["rev-parse", "--show-toplevel"]).await {
        Ok(output) => Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())),
        Err(error) if !succeeds(working_dir, &["rev-parse", "--is-inside-work-tree"]).await => {
            let _ = error;
            Err(GitDiffError::NotARepository)
        }
        Err(error) => Err(error),
    }
}

async fn succeeds(repo_root: &Path, args: &[&str]) -> bool {
    run_output(repo_root, args).await.is_ok()
}

fn non_empty(output: &Output) -> Option<String> {
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}
