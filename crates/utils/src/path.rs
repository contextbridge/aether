use std::path::{Path, PathBuf};

/// Renders `path` relative to the user's home directory (`~/…`) when it lives
/// inside it, mirroring the way shells display paths.
pub fn home_relative_path(path: &Path) -> String {
    home_dir().map_or_else(|| path.display().to_string(), |home| home_relative_path_with_home(path, &home))
}

pub fn aether_home() -> Option<PathBuf> {
    app_home("AETHER_HOME", ".aether")
}

pub fn app_home(env_var: &str, dot_dir: &str) -> Option<PathBuf> {
    resolve_home(std::env::var(env_var).ok().as_deref(), home_dir().as_deref(), dot_dir)
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn resolve_home(env_override: Option<&str>, home: Option<&Path>, dot_dir: &str) -> Option<PathBuf> {
    if let Some(value) = env_override
        && !value.trim().is_empty()
    {
        return Some(PathBuf::from(value));
    }

    home.map(|home| home.join(dot_dir))
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
    fn resolve_home_prefers_a_non_empty_env_override() {
        let home = Some(Path::new("/home/test"));
        assert_eq!(resolve_home(Some("/tmp/custom"), home, ".app"), Some(PathBuf::from("/tmp/custom")));
        assert_eq!(resolve_home(Some("  "), home, ".app"), Some(PathBuf::from("/home/test/.app")));
        assert_eq!(resolve_home(None, home, ".app"), Some(PathBuf::from("/home/test/.app")));
        assert_eq!(resolve_home(None, None, ".app"), None);
    }

    #[test]
    fn paths_outside_home_stay_absolute() {
        let home = Path::new("/home/user");
        assert_eq!(home_relative_path_with_home(Path::new("/etc/config"), home), "/etc/config");
        assert_eq!(home_relative_path_with_home(home.parent().unwrap(), home), "/home");
    }
}
