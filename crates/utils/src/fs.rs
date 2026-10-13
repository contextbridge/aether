use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = temp_path_for(path);
    let result = write_synced(&temp_path, bytes).and_then(|()| fs::rename(&temp_path, path));

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn temp_path_for(path: &Path) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp.{}.{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    path.with_file_name(name)
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
