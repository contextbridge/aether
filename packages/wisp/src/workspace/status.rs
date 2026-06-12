use std::path::Path;
use std::process::Stdio;

use utils::paths::home_relative_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub display_dir: String,
    pub git_ref: Option<String>,
}

impl WorkspaceStatus {
    pub fn new(display_dir: impl Into<String>, git_ref: Option<String>) -> Self {
        Self { display_dir: display_dir.into(), git_ref }
    }

    pub fn label(&self) -> String {
        self.git_ref
            .as_ref()
            .map_or_else(|| self.display_dir.clone(), |git_ref| format!("{} · {git_ref}", self.display_dir))
    }

    pub async fn resolve(cwd: &Path) -> Self {
        let display_dir = home_relative_path(cwd);
        let git_ref = resolve_git_ref(cwd).await;
        Self::new(display_dir, git_ref)
    }
}

async fn resolve_git_ref(cwd: &Path) -> Option<String> {
    if let Some(branch) = git_stdout(cwd, &["branch", "--show-current"]).await {
        return Some(branch);
    }
    git_stdout(cwd, &["rev-parse", "--short", "HEAD"]).await
}

async fn git_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_combines_dir_and_ref() {
        let status = WorkspaceStatus::new("~/code/aether-2", Some("main".to_string()));
        assert_eq!(status.label(), "~/code/aether-2 · main");
    }

    #[test]
    fn label_omits_ref_when_absent() {
        let status = WorkspaceStatus::new("~/scratch", None);
        assert_eq!(status.label(), "~/scratch");
    }
}
