use std::fs;
use std::io::{self, Write};
use std::path::Path;
use tempfile::NamedTempFile;

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    if let Ok(existing) = fs::metadata(path) {
        temp.as_file().set_permissions(existing.permissions())?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_atomic_creates_parents_and_replaces_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/file.json");

        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, ["file.json"], "temp files should not be left behind");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_creates_private_files_and_keeps_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file");
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

        write_atomic(&path, b"first").unwrap();
        assert_eq!(mode(&path), 0o600);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(mode(&path), 0o640);
    }

    #[test]
    fn write_atomic_failure_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("target");
        fs::create_dir_all(path.join("occupied")).unwrap();

        assert!(write_atomic(&path, b"data").is_err());

        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, ["target"], "the temp file should be removed after a failed rename");
    }
}
