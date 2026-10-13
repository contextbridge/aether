use std::fs::{File, OpenOptions, TryLockError, create_dir_all, remove_file};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};
use std::process;

/// A lockfile that tracks the daemon's PID
pub struct PidLockfile {
    path: PathBuf,
    _file: File,
}

impl PidLockfile {
    /// Acquire a lockfile, writing the current PID
    pub fn acquire(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        file.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => {
                io::Error::new(io::ErrorKind::WouldBlock, "Lockfile is held by another process")
            }
            TryLockError::Error(error) => error,
        })?;

        file.set_len(0)?;
        file.seek(io::SeekFrom::Start(0))?;
        write!(file, "{}", process::id())?;
        file.flush()?;

        Ok(Self { path: path.to_path_buf(), _file: file })
    }
}

impl Drop for PidLockfile {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lockfile_acquire_and_drop() {
        let temp = TempDir::new().unwrap();
        let lock_path = temp.path().join("test.lock");

        {
            let _lock = PidLockfile::acquire(&lock_path).unwrap();
            assert!(lock_path.exists());

            assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), process::id().to_string());
        }

        assert!(!lock_path.exists());
    }

    #[test]
    fn test_lockfile_blocks_second_acquire() {
        let temp = TempDir::new().unwrap();
        let lock_path = temp.path().join("test.lock");
        let _lock = PidLockfile::acquire(&lock_path).unwrap();
        let result = PidLockfile::acquire(&lock_path);

        assert_eq!(result.err().map(|error| error.kind()), Some(io::ErrorKind::WouldBlock));
    }
}
