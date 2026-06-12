use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub(crate) enum WorkspaceCloneError {
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("source is not a directory: {0}")]
    SourceNotADirectory(PathBuf),
    #[error("clone failed: {0}")]
    CloneFailed(String),
    #[error("copy-on-write cloning is not supported on this platform")]
    UnsupportedPlatform,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Clone `src` into `dest` using filesystem copy-on-write. Fails with
/// `CloneFailed` when the clone is unsupported or the copy fails.
pub(crate) async fn cow_clone_dir(src: &Path, dest: &Path) -> Result<(), WorkspaceCloneError> {
    if !src.is_dir() {
        return Err(WorkspaceCloneError::SourceNotADirectory(src.to_path_buf()));
    }
    if dest.symlink_metadata().is_ok() {
        return Err(WorkspaceCloneError::DestinationExists(dest.to_path_buf()));
    }

    #[cfg(target_os = "linux")]
    {
        use tokio::{fs::remove_dir_all, process::Command};

        let output = Command::new("cp")
            .arg("-a")
            .arg("--reflink=always")
            .arg("--")
            .arg(src)
            .arg(dest)
            .env("LC_ALL", "C")
            .output()
            .await?;

        if output.status.success() {
            return Ok(());
        }

        let _ = remove_dir_all(dest).await;
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(WorkspaceCloneError::CloneFailed(stderr))
    }

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;

        let src_c = std::ffi::CString::new(src.as_os_str().as_bytes())
            .map_err(|_| WorkspaceCloneError::CloneFailed("source path contains NUL".to_string()))?;
        let dest_c = std::ffi::CString::new(dest.as_os_str().as_bytes())
            .map_err(|_| WorkspaceCloneError::CloneFailed("destination path contains NUL".to_string()))?;
        let dest = dest.to_path_buf();
        tokio::task::spawn_blocking(move || {
            if unsafe { libc::clonefile(src_c.as_ptr(), dest_c.as_ptr(), 0) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            let _ = std::fs::remove_dir_all(&dest);
            match error.raw_os_error() {
                Some(libc::ENOTSUP | libc::EXDEV) => Err(WorkspaceCloneError::CloneFailed(error.to_string())),
                Some(libc::EEXIST) => Err(WorkspaceCloneError::DestinationExists(dest)),
                _ => Err(WorkspaceCloneError::Io(error)),
            }
        })
        .await
        .map_err(|e| WorkspaceCloneError::CloneFailed(e.to_string()))?
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(WorkspaceCloneError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clone_fails_when_source_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = cow_clone_dir(&dir.path().join("missing"), &dir.path().join("dest")).await;
        assert!(matches!(result, Err(WorkspaceCloneError::SourceNotADirectory(_))), "got: {result:?}");
    }

    #[tokio::test]
    async fn clone_fails_when_destination_exists() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(&dest).unwrap();

        let result = cow_clone_dir(&src, &dest).await;
        assert!(matches!(result, Err(WorkspaceCloneError::DestinationExists(_))), "got: {result:?}");
    }

    #[tokio::test]
    async fn clone_replicates_directory_tree_when_cow_is_supported() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("nested/file.txt"), "contents").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("nested/file.txt", src.join("link")).unwrap();
        }

        let dest = dir.path().join("dest");
        match cow_clone_dir(&src, &dest).await {
            Err(WorkspaceCloneError::CloneFailed(reason)) => {
                eprintln!("skipping: filesystem does not support copy-on-write clones ({reason})");
            }
            Err(WorkspaceCloneError::UnsupportedPlatform) => {
                eprintln!("skipping: platform does not support copy-on-write clones");
            }
            result => {
                result.unwrap();
                assert_eq!(std::fs::read_to_string(dest.join("nested/file.txt")).unwrap(), "contents");
                #[cfg(unix)]
                assert!(dest.join("link").symlink_metadata().unwrap().file_type().is_symlink());
            }
        }
    }
}
