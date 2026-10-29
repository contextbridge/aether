use clankerdiff_client::RemoteErrorCode;
use clankerdiff_core::{FileStatus, RepoPath, RepositoryAction, StageState};
use clankerdiff_git::GitRepository;
use client_core::testing::FakeGit;
use tempfile::TempDir;
use utils::testing::TestWorkspace;

#[tokio::test]
async fn real_and_fake_git_report_the_same_non_repository_errors() {
    let outside = TempDir::new().unwrap();
    let root = outside.path().to_path_buf();
    let fake = FakeGit::not_a_repository(&root);
    let real = GitRepository::discover(&root).await.unwrap_err();
    let fake = fake.apply(RepositoryAction::StageAll).unwrap_err();
    assert_eq!(fake.to_string(), real.to_string());
    assert_eq!(fake.code, RemoteErrorCode::Git);
}

#[tokio::test]
async fn real_and_fake_git_report_the_same_commit_errors() {
    let repo = TestWorkspace::new().git_init();
    let fake = FakeGit::new(repo.root());
    for message in ["  ", "nothing staged"] {
        let action = RepositoryAction::Commit { message: message.into() };
        let repository = GitRepository::discover(repo.root()).await.unwrap();
        let real = repository.apply(action.clone()).await.unwrap_err();
        let fake = fake.apply(action).unwrap_err();
        assert_eq!(fake.to_string(), real.to_string());
    }
}

#[tokio::test]
async fn staging_and_unstaging_everything_changes_both_indexes_alike() {
    let twins = Twins::committed(&[("src/a.rs", "a old\n"), ("src/b.rs", "b old\n")]);
    twins.write("src/a.rs", "a new\n");
    twins.write("src/b.rs", "b new\n");
    twins.add("src/c.rs", "c\n");

    twins.apply(RepositoryAction::StageAll).await;
    twins.assert_status(&[("src/a.rs", "M."), ("src/b.rs", "M."), ("src/c.rs", "A.")]);

    twins.apply(RepositoryAction::UnstageAll).await;
    twins.assert_status(&[("src/a.rs", ".M"), ("src/b.rs", ".M"), ("src/c.rs", "??")]);
}

#[tokio::test]
async fn a_commit_of_everything_leaves_both_trees_clean() {
    let twins = Twins::committed(&[("file.txt", "original\n")]);
    twins.write("file.txt", "changed\n");

    twins.apply(RepositoryAction::StageAll).await;
    twins.apply(RepositoryAction::Commit { message: "my commit message".into() }).await;

    twins.assert_status(&[("file.txt", "")]);
    assert_eq!(twins.real.git(&["log", "-1", "--format=%s"]), "my commit message");
    assert_eq!(twins.fake.commits().last().map(String::as_str), Some("my commit message"));
}

#[tokio::test]
async fn discarding_restores_tracked_files_and_removes_untracked_ones() {
    let twins = Twins::committed(&[("tracked.txt", "original\n"), ("deleted.txt", "kept\n")]);
    twins.write("tracked.txt", "changed\n");
    twins.remove("deleted.txt");
    twins.add("untracked.txt", "scratch\n");

    for (path, status) in [
        ("tracked.txt", FileStatus::Modified),
        ("deleted.txt", FileStatus::Deleted),
        ("untracked.txt", FileStatus::Untracked),
    ] {
        twins.apply(RepositoryAction::Discard { path: RepoPath::new(path).unwrap(), status }).await;
    }

    for (path, contents) in
        [("tracked.txt", Some("original\n")), ("deleted.txt", Some("kept\n")), ("untracked.txt", None)]
    {
        assert_eq!(twins.contents(path), (contents.map(String::from), contents.map(String::from)), "{path}");
    }
    twins.assert_status(&[("tracked.txt", ""), ("deleted.txt", ""), ("untracked.txt", "")]);
}

struct Twins {
    real: TestWorkspace,
    fake: FakeGit,
}

impl Twins {
    fn committed(files: &[(&str, &str)]) -> Self {
        let real = TestWorkspace::new().git_init();
        let fake = FakeGit::new(real.root());
        for (path, contents) in files {
            real.write(path, contents);
            fake.add_file(*path, contents);
        }
        real.commit("initial");
        fake.stage_all();
        fake.commit("initial").unwrap();
        Self { real, fake }
    }

    fn add(&self, path: &str, contents: &str) {
        self.real.write(path, contents);
        self.fake.add_file(path, contents);
    }

    fn write(&self, path: &str, contents: &str) {
        self.real.write(path, contents);
        self.fake.write_file(path, contents);
    }

    fn remove(&self, path: &str) {
        std::fs::remove_file(self.real.path(path)).unwrap();
        self.fake.remove_file(path);
    }

    async fn apply(&self, action: RepositoryAction) {
        GitRepository::discover(self.real.root()).await.unwrap().apply(action.clone()).await.unwrap();
        self.fake.apply(action).unwrap();
    }

    fn contents(&self, path: &str) -> (Option<String>, Option<String>) {
        let real = std::fs::read_to_string(self.real.path(path)).ok();
        let fake = self.fake.file(path).and_then(|file| file.contents).map(|bytes| String::from_utf8(bytes).unwrap());
        (real, fake)
    }

    fn assert_status(&self, expected: &[(&str, &str)]) {
        for (path, xy) in expected {
            assert_eq!(self.real_status(path), *xy, "real git, {path}");
            assert_eq!(fake_status(self.fake.status(path)), *xy, "FakeGit, {path}");
        }
    }

    fn real_status(&self, path: &str) -> String {
        let line = self.real.git(&["status", "--porcelain=v2", "--", path]);
        match line.split_once(' ') {
            Some(("?", _)) => "??".to_string(),
            Some((_, rest)) => rest[..2].to_string(),
            None => String::new(),
        }
    }
}

fn fake_status(status: Option<(FileStatus, StageState)>) -> String {
    let Some((status, stage)) = status else { return String::new() };
    let code = match status {
        FileStatus::Untracked => return "??".to_string(),
        FileStatus::Modified => 'M',
        FileStatus::Added => 'A',
        FileStatus::Deleted => 'D',
        FileStatus::Renamed => 'R',
        FileStatus::Copied => 'C',
    };
    match stage {
        StageState::Staged => format!("{code}."),
        StageState::Unstaged => format!(".{code}"),
        StageState::PartiallyStaged => format!("{code}M"),
    }
}
