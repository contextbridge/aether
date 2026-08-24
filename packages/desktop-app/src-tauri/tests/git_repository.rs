use desktop_app_lib::git::{DiffScope, FileStatus, GitRepository, StageState};
use std::process::Command;

struct Repo {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
}

impl Repo {
    fn init() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = Self { _dir: dir, root };
        repo.git(&["init", "--initial-branch=main"]);
        repo.git(&["config", "user.name", "Desktop Test"]);
        repo.git(&["config", "user.email", "desktop@example.com"]);
        repo
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git").current_dir(&self.root).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

#[tokio::test]
async fn snapshot_includes_tracked_and_synthetic_untracked_patches() {
    let repo = Repo::init();
    repo.write("tracked.txt", "before\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "initial"]);
    repo.write("tracked.txt", "after\n");
    repo.write("notes.txt", "new note\n");

    let snapshot = GitRepository::new(repo.root.clone()).snapshot(DiffScope::Both).await.unwrap();

    assert!(snapshot.patch.contains("diff --git a/tracked.txt b/tracked.txt"));
    assert!(snapshot.patch.contains("diff --git a/notes.txt b/notes.txt"));
    assert_eq!(snapshot.files.iter().find(|file| file.path == "notes.txt").unwrap().status, FileStatus::Untracked);
}

#[tokio::test]
async fn snapshot_expands_untracked_directories_into_files() {
    let repo = Repo::init();
    repo.write("nested/review/SKILL.md", "review instructions\n");

    let snapshot = GitRepository::new(repo.root.clone()).snapshot(DiffScope::Both).await.unwrap();

    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.files[0].path, "nested/review/SKILL.md");
    assert!(snapshot.patch.contains("diff --git a/nested/review/SKILL.md b/nested/review/SKILL.md"));
}

#[tokio::test]
async fn staging_and_committing_refreshes_stage_state() {
    let repo = Repo::init();
    repo.write("file.txt", "one\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "initial"]);
    repo.write("file.txt", "two\n");
    let repository = GitRepository::new(repo.root.clone());

    repository.stage(&["file.txt".to_string()]).await.unwrap();
    let staged = repository.snapshot(DiffScope::Both).await.unwrap();
    assert_eq!(staged.files[0].stage_state, StageState::Staged);

    repository.commit("update").await.unwrap();
    assert!(repository.snapshot(DiffScope::Both).await.unwrap().files.is_empty());
}

#[tokio::test]
async fn discard_rename_restores_the_original_path() {
    let repo = Repo::init();
    repo.write("old.txt", "original\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "initial"]);
    repo.git(&["mv", "old.txt", "new.txt"]);
    let repository = GitRepository::new(repo.root.clone());

    repository.discard("new.txt", Some("old.txt"), FileStatus::Renamed).await.unwrap();

    assert_eq!(std::fs::read_to_string(repo.root.join("old.txt")).unwrap(), "original\n");
    assert!(!repo.root.join("new.txt").exists());
    assert!(repository.snapshot(DiffScope::Both).await.unwrap().files.is_empty());
}

#[tokio::test]
async fn snapshot_reports_stats_for_non_ascii_paths() {
    let repo = Repo::init();
    repo.write("café.txt", "before\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "initial"]);
    repo.write("café.txt", "after\n");

    let snapshot = GitRepository::new(repo.root.clone()).snapshot(DiffScope::Both).await.unwrap();
    let file = snapshot.files.iter().find(|file| file.path == "café.txt").unwrap();

    assert_eq!(file.additions, Some(1));
    assert_eq!(file.deletions, Some(1));
}

#[tokio::test]
async fn discard_added_file_works_before_the_first_commit() {
    let repo = Repo::init();
    repo.write("new.txt", "new\n");
    let repository = GitRepository::new(repo.root.clone());
    repository.stage(&["new.txt".to_string()]).await.unwrap();

    repository.discard("new.txt", None, FileStatus::Added).await.unwrap();

    assert!(!repo.root.join("new.txt").exists());
    assert!(repository.snapshot(DiffScope::Both).await.unwrap().files.is_empty());
}

#[tokio::test]
async fn rejects_paths_outside_the_repository() {
    let repo = Repo::init();
    let repository = GitRepository::new(repo.root.clone());

    let error = repository.stage(&["../outside.txt".to_string()]).await.unwrap_err();

    assert!(error.to_string().contains("Invalid repository path"));
}

#[cfg(unix)]
#[tokio::test]
async fn untracked_symlinks_do_not_read_contents_outside_the_repository() {
    use std::os::unix::fs::symlink;

    let repo = Repo::init();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "outside secret\n").unwrap();
    symlink(outside.path(), repo.root.join("link.txt")).unwrap();

    let snapshot = GitRepository::new(repo.root.clone()).snapshot(DiffScope::Both).await.unwrap();

    assert!(!snapshot.patch.contains("outside secret"));
}
