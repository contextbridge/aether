use clankerdiff_git::GitRepository;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;
use wisp::git_review::{DiffDocument, DiffScope};

pub struct Repo {
    _dir: TempDir,
    pub root: PathBuf,
}

impl Repo {
    pub fn init() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = Self { _dir: dir, root };
        let output = Command::new("git")
            .current_dir(&repo.root)
            .args(["-c", "init.templateDir=", "init", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        repo.git(&["config", "user.name", "Contract Test"]);
        repo.git(&["config", "user.email", "contract@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "tag.gpgsign", "false"]);
        repo.git(&["config", "core.hooksPath", repo.root.join(".git/disabled-hooks").to_str().unwrap()]);
        repo
    }

    pub fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git").current_dir(&self.root).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    }

    pub fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    pub async fn load(&self, scope: DiffScope) -> DiffDocument {
        let repository = GitRepository::discover(&self.root).await.expect("discover repository");
        (*repository.snapshot_with_sources(scope).await.expect("load repository").document).clone()
    }
}
