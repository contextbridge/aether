use std::path::{Path, PathBuf};

/// Renders `path` relative to the user's home directory (`~/…`) when it lives
/// inside it, mirroring the way shells display paths.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_inside_home_shorten_to_tilde() {
        let home = Path::new("/home/user");
        assert_eq!(home_relative_path_with_home(&home.join("project"), home), "~/project");
        assert_eq!(home_relative_path_with_home(home, home), "~");
    }

    #[test]
    fn paths_outside_home_stay_absolute() {
        let home = Path::new("/home/user");
        assert_eq!(home_relative_path_with_home(Path::new("/etc/config"), home), "/etc/config");
        assert_eq!(home_relative_path_with_home(home.parent().unwrap(), home), "/home");
    }
}
