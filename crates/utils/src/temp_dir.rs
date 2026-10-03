use std::{
    env::temp_dir,
    fs::{create_dir_all, remove_dir_all},
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::Builder;
use uuid::Uuid;

#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        Self(temp_dir().join(format!("aether-{}", Uuid::new_v4())))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn save(&self, label: &str, text: &str) -> io::Result<PathBuf> {
        create_dir_all(&self.0)?;
        let label = label.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', "_");
        let mut file = Builder::new().prefix(&format!("{label}-")).suffix(".txt").tempfile_in(&self.0)?;
        file.write_all(text.as_bytes())?;
        Ok(file.keep()?.1)
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{create_dir_all, read_to_string, write};

    use super::*;

    #[test]
    fn each_dir_is_unique() {
        assert_ne!(TempDir::new().path(), TempDir::new().path());
    }

    #[test]
    fn dropping_the_dir_deletes_what_was_written_into_it() {
        let dir = TempDir::new();
        create_dir_all(dir.path()).unwrap();
        write(dir.path().join("saved.txt"), "output").unwrap();
        let path = dir.path().to_path_buf();

        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn save_creates_the_dir_and_keeps_labels_with_path_separators_inside_it() {
        let dir = TempDir::new();

        let saved = dir.save("../my/server__tool", "output").unwrap();

        assert_eq!(saved.parent(), Some(dir.path()));
        assert_eq!(read_to_string(saved).unwrap(), "output");
    }
}
