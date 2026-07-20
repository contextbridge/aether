use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub display_dir: String,
    pub git_ref: Option<String>,
}

impl WorkspaceStatus {
    pub fn new(display_dir: impl Into<String>, git_ref: Option<String>) -> Self {
        Self { display_dir: display_dir.into(), git_ref }
    }

    /// Creates the path portion of the status without touching the repository.
    /// Git metadata is resolved by the runtime's `Command::ResolveWorkspace`
    /// operation, keeping process execution outside session state.
    pub fn initial(cwd: &Path) -> Self {
        Self::new(home_relative_path(cwd), None)
    }
}

pub fn home_relative_path(path: &Path) -> String {
    home_dir().map_or_else(|| path.display().to_string(), |home| home_relative_path_with_home(path, &home))
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
