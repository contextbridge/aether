use crate::EvalHarnessError;
use crucible::Workspace;
use std::fs::{create_dir_all, write};

pub fn write_fixture_files(files: &[(&str, &str)]) -> Result<crucible::Workspace, EvalHarnessError> {
    let workspace = Workspace::empty()?;
    for (relative_path, contents) in files {
        let path = workspace.path().join(relative_path);
        if let Some(parent) = path.parent() {
            create_dir_all(parent)
                .map_err(|source| EvalHarnessError::WriteFixture { path: parent.to_path_buf(), source })?;
        }

        write(&path, contents).map_err(|source| EvalHarnessError::WriteFixture { path, source })?;
    }

    Ok(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_with_files_writes_nested_fixtures() {
        let workspace = write_fixture_files(&[("nested/file.txt", "hello")]).unwrap();
        let contents = std::fs::read_to_string(workspace.path().join("nested/file.txt")).unwrap();
        assert_eq!(contents, "hello");
    }
}
