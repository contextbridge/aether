use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
pub(super) use utils::fs::JsonFileError as RegistryError;
use utils::fs::{read_json, write_json_atomic};
use utils::path::aether_home;

/// Persistent registry of managed workspaces, grouped by repository identity.
///
/// Stored as a JSON array at `$AETHER_HOME/workspaces.json`.
pub(super) struct WorkspaceRegistry {
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkspaceRecord {
    pub path: PathBuf,
    pub repo_key: String,
}

impl WorkspaceRegistry {
    pub(super) fn new() -> io::Result<Self> {
        let home =
            aether_home().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Aether home directory not found"))?;
        Ok(Self::from_path(home.join("workspaces.json")))
    }

    pub(super) fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Adds `path` under `repo_key`, deduplicating by canonicalized path. An
    /// existing entry for the same path has its `repo_key` refreshed instead.
    pub(super) fn register(&self, repo_key: &str, path: &Path) -> Result<(), RegistryError> {
        let canonical = path.canonicalize().map_err(|e| self.io_error(e))?;
        let mut records = self.read()?;

        if let Some(existing) = records.iter_mut().find(|record| record.path == canonical) {
            if existing.repo_key == repo_key {
                return Ok(());
            }
            existing.repo_key = repo_key.to_string();
        } else {
            records.push(WorkspaceRecord { path: canonical, repo_key: repo_key.to_string() });
        }

        self.write(&records)
    }

    /// Returns every registered workspace for `repo_key`, pruning entries
    /// whose directory no longer exists.
    pub(super) fn workspaces_for(&self, repo_key: &str) -> Result<Vec<WorkspaceRecord>, RegistryError> {
        let records = self.read()?;
        let live: Vec<WorkspaceRecord> = records.iter().filter(|record| record.path.exists()).cloned().collect();
        if live.len() != records.len() {
            self.write(&live)?;
        }
        Ok(live.into_iter().filter(|record| record.repo_key == repo_key).collect())
    }

    fn read(&self) -> Result<Vec<WorkspaceRecord>, RegistryError> {
        read_json(&self.path).map(Option::unwrap_or_default)
    }

    fn write(&self, records: &[WorkspaceRecord]) -> Result<(), RegistryError> {
        write_json_atomic(&self.path, records)
    }

    fn io_error(&self, source: io::Error) -> RegistryError {
        RegistryError::Io { path: self.path.clone(), source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_registry() -> (TempDir, WorkspaceRegistry) {
        let dir = TempDir::new().unwrap();
        let registry = WorkspaceRegistry::from_path(dir.path().join("workspaces.json"));
        (dir, registry)
    }

    fn make_dir(base: &TempDir, name: &str) -> PathBuf {
        let path = base.path().join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn register_and_list_groups_by_repo_key() {
        let (dir, registry) = temp_registry();
        let a = make_dir(&dir, "a");
        let b = make_dir(&dir, "b");
        let other = make_dir(&dir, "other");

        registry.register("repo1", &a).unwrap();
        registry.register("repo1", &b).unwrap();
        registry.register("repo2", &other).unwrap();

        let repo1 = registry.workspaces_for("repo1").unwrap();
        let paths: Vec<&Path> = repo1.iter().map(|record| record.path.as_path()).collect();
        assert_eq!(paths, vec![a.canonicalize().unwrap(), b.canonicalize().unwrap()]);
        assert_eq!(registry.workspaces_for("repo2").unwrap().len(), 1);
    }

    #[test]
    fn register_same_path_twice_keeps_single_entry() {
        let (dir, registry) = temp_registry();
        let a = make_dir(&dir, "a");

        registry.register("repo1", &a).unwrap();
        registry.register("repo1", &a).unwrap();

        assert_eq!(registry.workspaces_for("repo1").unwrap().len(), 1);
    }

    #[test]
    fn register_updates_repo_key_for_existing_path() {
        let (dir, registry) = temp_registry();
        let a = make_dir(&dir, "a");

        registry.register("old", &a).unwrap();
        registry.register("new", &a).unwrap();

        assert_eq!(registry.workspaces_for("old").unwrap().len(), 0);
        assert_eq!(registry.workspaces_for("new").unwrap().len(), 1);
    }

    #[test]
    fn workspaces_for_prunes_deleted_directories() {
        let (dir, registry) = temp_registry();
        let a = make_dir(&dir, "a");
        let b = make_dir(&dir, "b");
        registry.register("repo1", &a).unwrap();
        registry.register("repo1", &b).unwrap();

        fs::remove_dir_all(&b).unwrap();

        let live = registry.workspaces_for("repo1").unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].path, a.canonicalize().unwrap());

        let reread = registry.workspaces_for("repo1").unwrap();
        assert_eq!(reread.len(), 1);
    }

    #[test]
    fn register_succeeds_while_a_temp_path_is_occupied() {
        let (dir, registry) = temp_registry();
        let a = make_dir(&dir, "a");
        fs::create_dir(dir.path().join("workspaces.json.tmp")).unwrap();
        registry.register("repo1", &a).unwrap();
        assert_eq!(registry.workspaces_for("repo1").unwrap().len(), 1);
    }

    #[test]
    fn missing_registry_file_lists_empty() {
        let (_dir, registry) = temp_registry();
        assert_eq!(registry.workspaces_for("repo1").unwrap().len(), 0);
    }
}
