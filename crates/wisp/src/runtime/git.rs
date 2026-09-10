use crate::command::GitCommand;
use crate::git_review::{GitDiffError, GitDiffEvent};
use crate::session::workspace_status::{WorkspaceStatus, home_relative_path};
use clankerdiff_git::GitRepository;
use std::path::Path;

pub async fn execute(command: GitCommand) -> GitDiffEvent {
    match command {
        GitCommand::Load { request_id, working_dir, scope } => {
            let result = async {
                let repository = GitRepository::discover(working_dir).await?;
                Ok(repository.snapshot_with_sources(scope).await?)
            }.await;
            GitDiffEvent::Loaded { request_id, result }
        }
        GitCommand::Apply { request_id, repo_root, action } => {
            let result = async {
                let repository = GitRepository::discover(repo_root).await?;
                repository.apply(action).await?;
                Ok::<_, GitDiffError>(())
            }.await;
            GitDiffEvent::ActionFinished { request_id, result }
        }
    }
}

pub async fn resolve_workspace_status(cwd: &Path) -> WorkspaceStatus {
    let git_ref = match git_ref(cwd, &["branch", "--show-current"]).await {
        Some(reference) => Some(reference),
        None => git_ref(cwd, &["rev-parse", "--short", "HEAD"]).await,
    };
    WorkspaceStatus::new(home_relative_path(cwd), git_ref)
}

async fn git_ref(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git").args(args).current_dir(cwd).output().await.ok()?;
    if !output.status.success() { return None; }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}
