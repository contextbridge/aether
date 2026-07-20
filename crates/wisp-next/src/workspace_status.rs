use std::path::{Path, PathBuf};
use std::process::Command;

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

    pub fn resolve(cwd: &Path) -> Self {
        Self::new(home_relative_path(cwd), resolve_git_ref(cwd))
    }
}

pub fn home_relative_path(path: &Path) -> String {
    home_dir().map_or_else(|| path.display().to_string(), |home| home_relative_path_with_home(path, &home))
}

fn resolve_git_ref(cwd: &Path) -> Option<String> {
    git_stdout(cwd, &["branch", "--show-current"]).or_else(|| git_stdout(cwd, &["rev-parse", "--short", "HEAD"]))
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
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
