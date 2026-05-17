use std::path::{Path, PathBuf};

use tokio::process::Command;

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
    let output = Command::new("git").args(args).current_dir(cwd).output().await.ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn home_relative_path(path: &Path) -> String {
    home_dir().map_or_else(|| path.display().to_string(), |home| home_relative_path_with_home(path, &home))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn home_relative_path_with_home(path: &Path, home: &Path) -> String {
    if path == home {
        return "~".to_string();
    }

    path.strip_prefix(home)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map_or_else(|| path.display().to_string(), |relative| format!("~/{}", relative.display()))
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

    #[test]
    fn home_relative_path_rewrites_home_child() {
        let path = Path::new("/Users/josh/code/aether-2");
        let home = Path::new("/Users/josh");
        assert_eq!(home_relative_path_with_home(path, home), "~/code/aether-2");
    }

    #[test]
    fn home_relative_path_handles_home_itself() {
        let home = Path::new("/Users/josh");
        assert_eq!(home_relative_path_with_home(home, home), "~");
    }

    #[test]
    fn home_relative_path_leaves_external_path_absolute() {
        let path = Path::new("/opt/work/aether-2");
        let home = Path::new("/Users/josh");
        assert_eq!(home_relative_path_with_home(path, home), "/opt/work/aether-2");
    }
}
