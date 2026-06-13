use super::{CloneError, DirectoryCloner};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Platform-agnostic directory cloner for tests: a plain recursive `cp -R`.
pub struct StdCopyCloner;

impl DirectoryCloner for StdCopyCloner {
    fn clone_dir(&self, src: &Path, dst: &Path) -> Result<(), CloneError> {
        let output = Command::new("cp").arg("-R").arg(src).arg(dst).output().map_err(|e| CloneError::Clone {
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
            source: e,
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CloneError::Clone {
                src: src.to_path_buf(),
                dst: dst.to_path_buf(),
                source: std::io::Error::other(stderr.trim().to_string()),
            });
        }

        Ok(())
    }
}

pub fn init_repo(parent: &Path, name: &str) -> PathBuf {
    let repo = parent.join(name);
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("committed.txt"), format!("original {name}\n")).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "initial"]);
    repo
}

pub fn clone_repo(src: &Path, dst: PathBuf) -> PathBuf {
    let output = Command::new("git").arg("clone").arg(src).arg(&dst).output().expect("git clone spawns");
    assert!(output.status.success(), "git clone failed: {}", String::from_utf8_lossy(&output.stderr));
    git(&dst, &["config", "user.email", "test@example.com"]);
    git(&dst, &["config", "user.name", "Test"]);
    dst
}

pub fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output().expect("git spawns");
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
}

pub fn git_status(repo: &Path) -> String {
    let output =
        Command::new("git").arg("-C").arg(repo).args(["status", "--porcelain"]).output().expect("git status spawns");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
