//! Contract tests that run the real `git` binary against a temporary
//! repository. Broad Git-review behavior is covered by the in-memory
//! `FakeGit`; these only guard that the subprocess boundary and its parsers
//! still agree with actual `git` output.

#[path = "support/git_repo.rs"]
mod git_repo;
use git_repo::Repo;

use clankerdiff_git::GitRepository;
use clankerdiff_ratatui::diff::{PatchLineKind, RepoPath, RepositoryAction};
use tempfile::TempDir;
use wisp::git_review::{DiffDocument, DiffScope, FileDiff, FileStatus, StageState};
use wisp::runtime::resolve_workspace_status;

fn file<'a>(document: &'a DiffDocument, path: &str) -> &'a FileDiff {
    document
        .files
        .iter()
        .find(|file| file.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} missing from {:?}", paths(document)))
}

fn paths(document: &DiffDocument) -> Vec<&str> {
    document.files.iter().map(|file| file.path.as_str()).collect()
}

#[tokio::test]
async fn real_and_fake_git_report_the_same_non_repository_errors() {
    let outside = TempDir::new().unwrap();
    let root = outside.path().to_path_buf();
    let mut fake = wisp::testing::FakeGit::not_a_repository(&root);
    let real = GitRepository::discover(&root).await.unwrap_err();
    let fake = fake.apply(RepositoryAction::StageAll).unwrap_err();
    assert_eq!(fake.to_string(), real.to_string());
    assert_eq!(std::mem::discriminant(&fake), std::mem::discriminant(&real));
}

#[tokio::test]
async fn real_and_fake_git_report_the_same_commit_errors() {
    let repo = Repo::init();
    let mut fake = wisp::testing::FakeGit::new(&repo.root);
    for message in ["  ", "nothing staged"] {
        let action = RepositoryAction::Commit { message: message.into() };
        let repository = GitRepository::discover(&repo.root).await.unwrap();
        let real = repository.apply(action.clone()).await.unwrap_err();
        let fake = fake.apply(action).unwrap_err();
        assert_eq!(fake.to_string(), real.to_string());
        assert_eq!(std::mem::discriminant(&fake), std::mem::discriminant(&real));
    }
}

#[tokio::test]
async fn load_parses_modified_staged_and_untracked_files() {
    let repo = Repo::init();
    repo.write("src/lib.rs", "fn one() {}\nfn two() {}\n");
    repo.write("staged.txt", "original\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "init"]);

    repo.write("src/lib.rs", "fn one() {}\nfn two() { changed(); }\n");
    repo.write("staged.txt", "staged change\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("untracked.txt", "brand new\n");

    let document = repo.load(DiffScope::Both).await;
    assert_eq!(document.repo_root, repo.root.to_string_lossy());

    let modified = file(&document, "src/lib.rs");
    assert_eq!(modified.status, FileStatus::Modified);
    assert_eq!(modified.staged, StageState::Unstaged);
    let added: Vec<&str> = modified
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| line.kind == PatchLineKind::Added)
        .map(|line| line.text.as_ref())
        .collect();
    assert_eq!(added, ["fn two() { changed(); }"]);
    let removed = modified
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .find(|line| line.kind == PatchLineKind::Removed)
        .expect("the old line must appear as removed");
    assert_eq!(removed.text.as_ref(), "fn two() {}");
    assert_eq!(removed.old_line_no, Some(2));

    let staged = file(&document, "staged.txt");
    assert_eq!(staged.staged, StageState::Staged);

    let untracked = file(&document, "untracked.txt");
    assert_eq!(untracked.status, FileStatus::Untracked);
    assert!(
        untracked
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .any(|line| line.kind == PatchLineKind::Added && line.text.as_ref() == "brand new"),
        "untracked contents must render as additions"
    );
}

#[tokio::test]
async fn stage_commit_round_trip_reaches_a_clean_tree() {
    let repo = Repo::init();
    repo.write("file.txt", "one\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "init"]);
    repo.write("file.txt", "two\n");

    let repository = GitRepository::discover(&repo.root).await.unwrap();
    repository.apply(RepositoryAction::StagePaths(vec![RepoPath::new("file.txt").unwrap()])).await.unwrap();
    let document = repo.load(DiffScope::Both).await;
    assert_eq!(file(&document, "file.txt").staged, StageState::Staged);

    repository.apply(RepositoryAction::UnstagePaths(vec![RepoPath::new("file.txt").unwrap()])).await.unwrap();
    let document = repo.load(DiffScope::Both).await;
    assert_eq!(file(&document, "file.txt").staged, StageState::Unstaged);

    repository.apply(RepositoryAction::StageAll).await.unwrap();
    repository.apply(RepositoryAction::Commit { message: "update".to_string() }).await.unwrap();
    let document = repo.load(DiffScope::Both).await;
    assert!(document.files.is_empty(), "committed tree must be clean, found {:?}", paths(&document));
}

#[tokio::test]
async fn renames_and_binary_files_survive_parsing() {
    let repo = Repo::init();
    repo.write("old_name.rs", "fn kept() {}\n");
    repo.write("image.bin", [0u8, 159, 146, 150]);
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "init"]);

    repo.git(&["mv", "old_name.rs", "new_name.rs"]);
    repo.write("image.bin", [255u8, 216, 255, 0]);

    let document = repo.load(DiffScope::Both).await;

    let renamed = file(&document, "new_name.rs");
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(renamed.old_path.as_ref().map(RepoPath::as_str), Some("old_name.rs"));

    let binary = file(&document, "image.bin");
    assert!(binary.binary, "binary change must be flagged");
}

#[tokio::test]
async fn workspace_status_reports_the_current_branch() {
    let repo = Repo::init();
    repo.write("file.txt", "content\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "init"]);

    let status = resolve_workspace_status(&repo.root).await;
    assert_eq!(status.git_ref.as_deref(), Some("main"));

    let outside = TempDir::new().unwrap();
    let status = resolve_workspace_status(outside.path()).await;
    assert_eq!(status.git_ref, None, "a non-repository must resolve without a git ref");
}
