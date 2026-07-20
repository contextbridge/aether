//! Contract tests that run the real `git` binary against a temporary
//! repository. Broad Git-review behavior is covered by the in-memory
//! `FakeGit`; these only guard that the subprocess boundary and its parsers
//! still agree with actual `git` output.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;
use wisp::command::GitCommand;
use wisp::git_review::{DiffDocument, DiffScope, FileDiff, FileStatus, GitDiffEvent, PatchLineKind, StageState};
use wisp::request::RequestId;
use wisp::runtime::{execute_git, resolve_workspace_status};

struct Repo {
    _dir: TempDir,
    root: PathBuf,
}

impl Repo {
    fn init() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = Self { _dir: dir, root };
        repo.git(&["init", "--initial-branch=main"]);
        repo.git(&["config", "user.name", "Contract Test"]);
        repo.git(&["config", "user.email", "contract@example.com"]);
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

    async fn load(&self, scope: DiffScope) -> DiffDocument {
        let command =
            GitCommand::Load { request_id: RequestId::from(1), working_dir: self.root.clone(), repo_root: None, scope };
        match execute_git(command).await {
            GitDiffEvent::Loaded { result, .. } => result.expect("load must succeed against a real repository"),
            event => panic!("expected Loaded, got {event:?}"),
        }
    }
}

fn file<'a>(document: &'a DiffDocument, path: &str) -> &'a FileDiff {
    document
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} missing from {:?}", paths(document)))
}

fn paths(document: &DiffDocument) -> Vec<&str> {
    document.files.iter().map(|file| file.path.as_str()).collect()
}

async fn run_action(command: GitCommand) {
    match execute_git(command).await {
        GitDiffEvent::ActionFinished { result, .. } => result.expect("action must succeed"),
        event => panic!("expected ActionFinished, got {event:?}"),
    }
}

fn request(id: u64) -> RequestId {
    RequestId::from(id)
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
    assert_eq!(document.repo_root, repo.root);

    let modified = file(&document, "src/lib.rs");
    assert_eq!(modified.status, FileStatus::Modified);
    assert_eq!(modified.staged, StageState::Unstaged);
    let added: Vec<&str> = modified
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| line.kind == PatchLineKind::Added)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(added, ["fn two() { changed(); }"]);
    let removed = modified
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .find(|line| line.kind == PatchLineKind::Removed)
        .expect("the old line must appear as removed");
    assert_eq!(removed.text, "fn two() {}");
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
            .any(|line| line.kind == PatchLineKind::Added && line.text == "brand new"),
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

    run_action(GitCommand::StageFiles {
        request_id: request(2),
        repo_root: repo.root.clone(),
        paths: vec!["file.txt".to_string()],
    })
    .await;
    let document = repo.load(DiffScope::Both).await;
    assert_eq!(file(&document, "file.txt").staged, StageState::Staged);

    run_action(GitCommand::UnstageFiles {
        request_id: request(3),
        repo_root: repo.root.clone(),
        paths: vec!["file.txt".to_string()],
    })
    .await;
    let document = repo.load(DiffScope::Both).await;
    assert_eq!(file(&document, "file.txt").staged, StageState::Unstaged);

    run_action(GitCommand::StageAll { request_id: request(4), repo_root: repo.root.clone() }).await;
    run_action(GitCommand::Commit {
        request_id: request(5),
        repo_root: repo.root.clone(),
        message: "update".to_string(),
    })
    .await;
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
    assert_eq!(renamed.old_path.as_deref(), Some("old_name.rs"));

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
