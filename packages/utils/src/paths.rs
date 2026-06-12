use std::path::{Path, PathBuf};

pub fn home_relative_path(path: &Path) -> String {
    home_dir().map_or_else(|| path.display().to_string(), |home| home_relative_path_with_home(path, &home))
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

pub fn home_relative_path_with_home(path: &Path, home: &Path) -> String {
    if path == home {
        return "~".to_string();
    }

    path.strip_prefix(home)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map_or_else(|| path.display().to_string(), |relative| format!("~/{}", relative.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_relative_path_rewrites_home_child() {
        let path = Path::new("/Users/josh/code/aether-2");
        let home = Path::new("/Users/josh");
        assert_eq!(home_relative_path_with_home(path, home), "~/code/aether-2");
    }

    #[test]
    fn home_relative_path_handles_home_itself() {
        let home = Path::new("/Users/josh");
        assert_eq!(home_relative_path_with_home(home, home), "~");
    }

    #[test]
    fn home_relative_path_leaves_external_path_absolute() {
        let path = Path::new("/opt/work/aether-2");
        let home = Path::new("/Users/josh");
        assert_eq!(home_relative_path_with_home(path, home), "/opt/work/aether-2");
    }
}
