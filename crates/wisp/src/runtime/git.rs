use crate::session::workspace_status::{WorkspaceStatus, home_relative_path};
use std::path::Path;

pub async fn resolve_workspace_status(cwd: &Path) -> WorkspaceStatus {
    let git_ref = match git_ref(cwd, &["branch", "--show-current"]).await {
        Some(reference) => Some(reference),
        None => git_ref(cwd, &["rev-parse", "--short", "HEAD"]).await,
    };
    WorkspaceStatus::new(home_relative_path(cwd), git_ref)
}

async fn git_ref(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git").args(args).current_dir(cwd).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}
