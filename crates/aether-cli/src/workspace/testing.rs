use super::{CloneError, DirectoryCloner};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use utils::testing::{git_commit, git_init};

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
    fs::write(repo.join("committed.txt"), format!("original {name}\n")).unwrap();
    git_init(&repo);
    git_commit(&repo, "initial");
    repo
}
