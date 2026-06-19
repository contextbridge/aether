use std::fs::{File, OpenOptions, create_dir_all, remove_file};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

/// A lockfile that tracks the daemon's PID
pub struct PidLockfile {
    path: PathBuf,
    _file: File,
}

impl PidLockfile {
    /// Acquire a lockfile, writing the current PID
    ///
    /// Uses `flock` for advisory locking. Only one process can hold the lock.
    pub fn acquire(path: &Path) -> io::Result<Self> {
        use std::io::Seek;

        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }

        let file = {
            let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;

            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if result != 0 {
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, "Lockfile is held by another process"));
                }
            }

            file.set_len(0)?;
            file.seek(io::SeekFrom::Start(0))?;
            write!(file, "{}", process::id())?;
            file.flush()?;
            file
        };

        Ok(Self { path: path.to_path_buf(), _file: file })
    }
}

impl Drop for PidLockfile {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

#[cfg(test)]
fn read_pid(path: &Path) -> Option<u32> {
    use std::io::Read;

    let mut file = File::open(path).ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    contents.trim().parse().ok()
}

#[cfg(test)]
fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid.cast_signed(), 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

impl PidLockfile {
    #[cfg(test)]
    pub fn is_stale(path: &Path) -> bool {
        let Some(pid) = read_pid(path) else {
            return true;
        };
        !is_process_running(pid)
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

            let pid = read_pid(&lock_path).unwrap();
            assert_eq!(pid, process::id());
        }

        assert!(!lock_path.exists());
    }

    #[test]
    fn test_lockfile_blocks_second_acquire() {
        let temp = TempDir::new().unwrap();
        let lock_path = temp.path().join("test.lock");
        let _lock = PidLockfile::acquire(&lock_path).unwrap();
        let result = PidLockfile::acquire(&lock_path);

        assert!(result.is_err());
    }

    #[test]
    fn test_is_stale_nonexistent() {
        let temp = TempDir::new().unwrap();
        let lock_path = temp.path().join("nonexistent.lock");

        assert!(PidLockfile::is_stale(&lock_path));
    }

    #[test]
    fn test_is_process_running_self() {
        let pid = process::id();
        assert!(is_process_running(pid));
    }

    #[test]
    fn test_is_process_running_invalid() {
        let fake_pid = 4_000_000_000u32;
        assert!(!is_process_running(fake_pid));
    }
}
