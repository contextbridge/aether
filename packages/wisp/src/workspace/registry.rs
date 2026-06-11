use super::git::run_git;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use utils::settings::{SettingsStore, load_json_or_default, save_json};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedWorkspace {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub repo: Option<String>,
    pub created_at: String,
}

impl ManagedWorkspace {
    pub fn new(name: String, path: PathBuf, repo: Option<String>) -> Self {
        Self { name, path, repo, created_at: chrono::Utc::now().to_rfc3339() }
    }

    pub fn for_dir(path: &Path, repo: Option<String>) -> Self {
        Self::new(workspace_display_name(path), path.to_path_buf(), repo)
    }
}

/// Return the root commit for the git repository at `path`.
///
/// This deliberately avoids remote URL normalization so local copy-on-write siblings and
/// clones share identity by construction. Shallow clones without the root commit
/// and multi-root worktrees can produce distinct identities.
pub async fn repo_identity(path: &Path) -> Option<String> {
    run_git(path, &["rev-list", "--max-parents=0", "HEAD"])
        .await
        .ok()
        .and_then(|out| out.lines().next().map(str::to_string))
        .filter(|line| !line.is_empty())
}

/// Persistent record of workspaces the user has forked from or into, stored in
/// `~/.aether/workspaces.json` so they can be offered in future fork menus.
#[derive(Clone)]
pub struct WorkspaceRegistry {
    path: PathBuf,
}

impl WorkspaceRegistry {
    pub fn new() -> Option<Self> {
        SettingsStore::new("AETHER_HOME", ".aether").map(|store| Self::from_path(store.home().join("workspaces.json")))
    }

    pub fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// All registered workspaces whose directory still exists. Dead entries
    /// are hidden rather than deleted so a workspace on an unmounted volume
    /// reappears once the directory does.
    pub fn list(&self) -> Vec<ManagedWorkspace> {
        self.read_all().into_iter().filter(|workspace| workspace.path.is_dir()).collect()
    }

    /// Add a workspace unless its path is already registered.
    pub fn register(&self, workspace: ManagedWorkspace) -> io::Result<()> {
        let mut workspaces = self.read_all();
        if workspaces.iter().any(|existing| existing.path == workspace.path) {
            return Ok(());
        }
        workspaces.push(workspace);
        self.write_all(workspaces)
    }

    fn read_all(&self) -> Vec<ManagedWorkspace> {
        load_json_or_default::<RegistryFile>(&self.path).workspaces
    }

    fn write_all(&self, workspaces: Vec<ManagedWorkspace>) -> io::Result<()> {
        save_json(&self.path, &RegistryFile { workspaces })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    workspaces: Vec<ManagedWorkspace>,
}

fn workspace_display_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn registry(dir: &TempDir) -> WorkspaceRegistry {
        WorkspaceRegistry::from_path(dir.path().join("workspaces.json"))
    }

    fn workspace(dir: &TempDir, name: &str) -> ManagedWorkspace {
        let path = dir.path().join(name);
        fs::create_dir_all(&path).unwrap();
        ManagedWorkspace {
            name: name.to_string(),
            path,
            repo: Some("repo".to_string()),
            created_at: "2026-06-11T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn register_and_list_roundtrip() {
        let dir = TempDir::new().unwrap();
        let registry = registry(&dir);
        let first = workspace(&dir, "first");
        let second = workspace(&dir, "second");

        registry.register(first.clone()).unwrap();
        registry.register(second.clone()).unwrap();

        assert_eq!(registry.list(), vec![first, second]);
    }

    #[test]
    fn register_ignores_already_registered_path() {
        let dir = TempDir::new().unwrap();
        let registry = registry(&dir);
        let original = workspace(&dir, "repo");

        registry.register(original.clone()).unwrap();
        let renamed = ManagedWorkspace { name: "other-name".to_string(), ..original.clone() };
        registry.register(renamed).unwrap();

        assert_eq!(registry.list(), vec![original]);
    }

    #[test]
    fn list_hides_missing_paths_without_rewriting_file() {
        let dir = TempDir::new().unwrap();
        let registry = registry(&dir);
        let kept = workspace(&dir, "kept");
        let removed = workspace(&dir, "removed");
        registry.register(kept.clone()).unwrap();
        registry.register(removed.clone()).unwrap();

        fs::remove_dir_all(&removed.path).unwrap();
        assert_eq!(registry.list(), vec![kept.clone()]);

        fs::create_dir_all(&removed.path).unwrap();
        assert_eq!(registry.list(), vec![kept, removed]);
    }

    #[test]
    fn list_returns_empty_for_missing_or_malformed_file() {
        let dir = TempDir::new().unwrap();
        assert!(registry(&dir).list().is_empty());

        fs::write(dir.path().join("workspaces.json"), "{not-json").unwrap();
        assert!(registry(&dir).list().is_empty());
    }

    #[test]
    fn register_leaves_no_temp_files() {
        let dir = TempDir::new().unwrap();
        let registry = registry(&dir);
        registry.register(workspace(&dir, "repo")).unwrap();

        let temp_count = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(temp_count, 0);
    }

    fn init_git_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        std::process::Command::new("git").arg("-C").arg(path).args(["init"]).output().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test User"])
            .output()
            .unwrap();
    }

    fn commit_all(path: &Path) {
        std::process::Command::new("git").arg("-C").arg(path).args(["add", "."]).output().unwrap();
        std::process::Command::new("git").arg("-C").arg(path).args(["commit", "-m", "initial"]).output().unwrap();
    }

    #[tokio::test]
    async fn repo_identity_matches_across_clone() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        init_git_repo(&src);
        fs::write(src.join("file.txt"), "base").unwrap();
        commit_all(&src);
        std::process::Command::new("git").arg("clone").arg(&src).arg(&dest).output().unwrap();

        assert_eq!(repo_identity(&src).await, repo_identity(&dest).await);
    }

    #[tokio::test]
    async fn repo_identity_differs_across_unrelated_repos() {
        let dir = TempDir::new().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        init_git_repo(&first);
        fs::write(first.join("file.txt"), "first").unwrap();
        commit_all(&first);
        init_git_repo(&second);
        fs::write(second.join("file.txt"), "second").unwrap();
        commit_all(&second);

        assert_ne!(repo_identity(&first).await, repo_identity(&second).await);
    }

    #[tokio::test]
    async fn repo_identity_returns_none_for_non_git_dir() {
        let dir = TempDir::new().unwrap();
        assert_eq!(repo_identity(dir.path()).await, None);
    }
}
