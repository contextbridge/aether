use crate::WorkspaceError;
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    fs::{create_dir_all, write},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

pub struct Workspace {
    path: PathBuf,
    root_path: PathBuf,
    relative_cwd: Option<PathBuf>,
    temp_dir: TempDir,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedWorkspaceInfo {
    pub root_path: PathBuf,
    pub path: PathBuf,
}

impl Workspace {
    pub fn empty() -> Result<Self, WorkspaceError> {
        Self::from_temp_dir(new_temp_dir()?, None)
    }

    pub fn from_dir(src_path: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let src_path = src_path.into();
        let temp_dir = new_temp_dir()?;

        copy_dir_contents(&src_path, temp_dir.path()).map_err(|source| WorkspaceError::CopyFixture {
            from: src_path.clone(),
            to: temp_dir.path().to_path_buf(),
            source,
        })?;

        Self::from_temp_dir(temp_dir, None)
    }

    pub fn from_files<T: AsRef<Path>, U: AsRef<str>>(
        files: impl IntoIterator<Item = (T, U)>,
    ) -> Result<Self, WorkspaceError> {
        let workspace = Self::empty()?;
        for (relative_path, contents) in files {
            let path = workspace.path().join(relative_path.as_ref());
            if let Some(parent) = path.parent() {
                create_dir_all(parent)
                    .map_err(|source| WorkspaceError::WriteFile { path: parent.to_path_buf(), source })?;
            }
            write(&path, contents.as_ref()).map_err(|source| WorkspaceError::WriteFile { path, source })?;
        }
        Ok(workspace)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, relative_path: impl AsRef<Path>) -> PathBuf {
        self.path.join(relative_path)
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn relative_cwd(&self) -> Option<&Path> {
        self.relative_cwd.as_deref()
    }

    /// Prevents the workspace from getting automatically removed and returns its retained root and effective cwd. The caller is responsible for cleanup.
    pub fn persist(self) -> RetainedWorkspaceInfo {
        let root_path = self.temp_dir.keep();
        let path = self.relative_cwd.map_or_else(|| root_path.clone(), |relative_cwd| root_path.join(relative_cwd));
        RetainedWorkspaceInfo { root_path, path }
    }

    pub(super) fn from_temp_dir(temp_dir: TempDir, subdir: Option<PathBuf>) -> Result<Self, WorkspaceError> {
        let root_path = temp_dir.path().to_path_buf();
        let (path, relative_cwd) = resolve_subdir(&root_path, subdir)?;
        Ok(Self { path, root_path, relative_cwd, temp_dir })
    }
}

pub(super) fn new_temp_dir() -> Result<TempDir, WorkspaceError> {
    tempfile::tempdir().map_err(WorkspaceError::CreateTempDir)
}

fn resolve_subdir(root_path: &Path, subdir: Option<PathBuf>) -> Result<(PathBuf, Option<PathBuf>), WorkspaceError> {
    match subdir {
        None => Ok((root_path.to_path_buf(), None)),
        Some(relative_cwd) => {
            let working_path = root_path.join(&relative_cwd);
            if !working_path.exists() {
                return Err(WorkspaceError::MissingSubdir { path: working_path });
            }
            Ok((working_path, Some(relative_cwd)))
        }
    }
}

fn copy_dir_contents(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            copy_dir_contents(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{read_to_string, remove_dir_all};

    #[test]
    fn persist_reports_root_and_path_for_local_workspace() {
        let workspace = Workspace::from_files([("notes.txt", "hi\n")]).unwrap();

        let retained = workspace.persist();

        assert_eq!(retained.root_path, retained.path);
        assert_eq!(read_to_string(retained.path.join("notes.txt")).unwrap(), "hi\n");
        remove_dir_all(retained.root_path).unwrap();
    }
}
