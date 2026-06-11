use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Debug, thiserror::Error)]
pub(crate) enum GitCommandError {
    #[error("git command failed in {cwd}: git {args}: {stderr}")]
    Failed { cwd: PathBuf, args: String, stderr: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub(crate) async fn run_git(cwd: &Path, args: &[&str]) -> Result<String, GitCommandError> {
    let output = run_git_bytes(cwd, args).await?;
    Ok(String::from_utf8_lossy(&output).to_string())
}

pub(crate) async fn run_git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, GitCommandError> {
    let output = tokio::process::Command::new("git").arg("-C").arg(cwd).args(args).env("LC_ALL", "C").output().await?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitCommandError::Failed {
            cwd: cwd.to_path_buf(),
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub(crate) async fn run_git_with_stdin(cwd: &Path, args: &[&str], stdin: &[u8]) -> Result<String, GitCommandError> {
    let mut child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut child_stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        child_stdin.write_all(stdin).await?;
    }

    let output = child.wait_with_output().await?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(GitCommandError::Failed {
            cwd: cwd.to_path_buf(),
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub(crate) async fn repo_root(cwd: &Path) -> Option<PathBuf> {
    run_git(cwd, &["rev-parse", "--show-toplevel"])
        .await
        .ok()
        .map(|out| PathBuf::from(out.trim()))
        .filter(|path| !path.as_os_str().is_empty())
}
