use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

pub struct SettingsStore {
    home: PathBuf,
}

impl SettingsStore {
    pub fn new(env_override: &str, dot_dir: &str) -> Option<Self> {
        let home = resolve_home(
            std::env::var(env_override).ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
            std::env::var("USERPROFILE").ok().as_deref(),
            dot_dir,
        )?;
        Some(Self { home })
    }

    pub fn from_path(home: &Path) -> Self {
        Self { home: home.to_path_buf() }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn load_or_create<T: Serialize + DeserializeOwned + Default>(&self) -> T {
        load_or_create_at(&self.home.join("settings.json"))
    }

    pub fn save<T: Serialize>(&self, settings: &T) -> io::Result<()> {
        save_to_path(&self.home.join("settings.json"), settings)
    }
}

pub fn resolve_home(
    env_override: Option<&str>,
    home: Option<&str>,
    userprofile: Option<&str>,
    dot_dir: &str,
) -> Option<PathBuf> {
    if let Some(value) = env_override
        && !value.trim().is_empty()
    {
        return Some(PathBuf::from(value));
    }

    let fallback_home = home.or(userprofile)?;
    Some(PathBuf::from(fallback_home).join(dot_dir))
}

fn load_or_create_at<T: Serialize + DeserializeOwned + Default>(path: &Path) -> T {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let defaults = T::default();
            if let Err(error) = save_to_path(path, &defaults) {
                warn!("Failed to write default settings to {}: {error}", path.display());
            }
            return defaults;
        }
        Err(error) => {
            warn!("Failed reading settings {}: {error}", path.display());
            return T::default();
        }
    };

    match serde_json::from_str::<T>(&raw) {
        Ok(settings) => settings,
        Err(error) => {
            warn!("Malformed settings JSON at {}: {error}", path.display());
            T::default()
        }
    }
}

fn save_to_path<T: Serialize>(path: &Path, settings: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = temp_path_for(path);
    let serialized = serde_json::to_vec_pretty(settings)
        .map_err(|error| io::Error::other(format!("Failed to serialize settings: {error}")))?;

    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&serialized)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }

    fs::rename(&temp_path, path)?;
    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    let pid = std::process::id();
    path.with_extension(format!("json.tmp.{pid}.{nanos}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use tempfile::TempDir;

    #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
    struct FakeSettings {
        name: String,
    }

    #[test]
    fn creates_defaults_when_missing() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("settings.json");

        let settings: FakeSettings = load_or_create_at(&path);

        assert_eq!(settings, FakeSettings::default());
        assert!(path.exists());
    }

    #[test]
    fn round_trip_serde() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("settings.json");
        let settings = FakeSettings { name: "test".to_string() };

        save_to_path(&path, &settings).unwrap();
        let loaded: FakeSettings = load_or_create_at(&path);

        assert_eq!(loaded, settings);
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("settings.json");
        fs::write(&path, "{not-json").unwrap();

        let loaded: FakeSettings = load_or_create_at(&path);

        assert_eq!(loaded, FakeSettings::default());
    }

    #[test]
    fn resolve_home_prefers_env_override() {
        let resolved = resolve_home(Some("/tmp/custom"), Some("/home/test"), None, ".app").unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/custom"));
    }

    #[test]
    fn resolve_home_uses_home_fallback() {
        let resolved = resolve_home(None, Some("/home/test"), None, ".app").unwrap();
        assert_eq!(resolved, PathBuf::from("/home/test/.app"));
    }

    #[test]
    fn resolve_home_uses_userprofile_fallback() {
        let resolved = resolve_home(None, None, Some("C:\\Users\\test"), ".app").unwrap();
        assert_eq!(resolved, PathBuf::from("C:\\Users\\test/.app"));
    }

    #[test]
    fn resolve_home_ignores_empty_override() {
        let resolved = resolve_home(Some("  "), Some("/home/test"), None, ".app").unwrap();
        assert_eq!(resolved, PathBuf::from("/home/test/.app"));
    }

    #[test]
    fn resolve_home_returns_none_when_no_home() {
        assert!(resolve_home(None, None, None, ".app").is_none());
    }

    #[test]
    fn atomic_save_overwrites_and_cleans_temp_files() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("settings.json");

        let first = FakeSettings { name: "first".to_string() };
        save_to_path(&path, &first).unwrap();

        let second = FakeSettings { name: "second".to_string() };
        save_to_path(&path, &second).unwrap();

        let loaded: FakeSettings = load_or_create_at(&path);
        assert_eq!(loaded, second);

        let temp_count = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(temp_count, 0, "temporary files should be cleaned up");
    }

    #[test]
    fn settings_store_load_and_save() {
        let temp_dir = TempDir::new().unwrap();
        let store = SettingsStore::from_path(temp_dir.path());

        let settings: FakeSettings = store.load_or_create();
        assert_eq!(settings, FakeSettings::default());

        let updated = FakeSettings { name: "updated".to_string() };
        store.save(&updated).unwrap();

        let loaded: FakeSettings = store.load_or_create();
        assert_eq!(loaded, updated);
    }
}
