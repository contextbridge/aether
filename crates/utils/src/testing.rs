use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub struct TestWorkspace {
    root: TempDir,
}

impl Default for TestWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl TestWorkspace {
    pub fn new() -> Self {
        Self { root: TempDir::new().expect("failed to create temp workspace") }
    }

    pub fn file(self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Self {
        self.write(path, contents);
        self
    }

    pub fn files<P: AsRef<Path>, C: AsRef<[u8]>>(self, files: impl IntoIterator<Item = (P, C)>) -> Self {
        files.into_iter().fold(self, |workspace, (path, contents)| workspace.file(path, contents))
    }

    pub fn dir(self, path: impl AsRef<Path>) -> Self {
        fs::create_dir_all(self.path(path)).expect("failed to create directory");
        self
    }

    #[cfg(unix)]
    pub fn symlink(self, target: impl AsRef<Path>, link: impl AsRef<Path>) -> Self {
        symlink(self.path(target), self.path(link)).expect("failed to create symlink");
        self
    }

    pub fn write(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }
        fs::write(&path, contents).expect("failed to write file");
        path
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn root_string(&self) -> String {
        self.root().to_string_lossy().into_owned()
    }

    pub fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        self.root().join(path)
    }

    pub fn path_string(&self, path: impl AsRef<Path>) -> String {
        self.path(path).to_string_lossy().into_owned()
    }

    pub fn git_init(self) -> Self {
        git_init(self.root());
        self
    }

    pub fn commit(&self, message: &str) -> String {
        git_commit(self.root(), message)
    }

    pub fn git(&self, args: &[&str]) -> String {
        git(self.root(), args)
    }
}

pub fn git_init(dir: &Path) {
    git(dir, &["-c", "init.templateDir=", "init", "--quiet", "--initial-branch=main"]);
    for (key, value) in [
        ("user.name", "Test"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(dir, &["config", key, value]);
    }
}

pub fn git_commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("failed to spawn git");
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
