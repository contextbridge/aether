use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("failed to spawn `git {args}`: {source}")]
    Spawn { args: String, source: std::io::Error },
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
}

pub fn git<T: AsRef<OsStr>>(repo: Option<&Path>, args: &[T]) -> Result<Vec<u8>, GitError> {
    let output = command(repo, args).output().map_err(|source| spawn_error(args, source))?;
    stdout(args, output)
}

pub fn git_with_stdin<T: AsRef<OsStr>>(repo: Option<&Path>, args: &[T], stdin: &[u8]) -> Result<Vec<u8>, GitError> {
    let mut child = command(repo, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| spawn_error(args, source))?;
    child.stdin.take().expect("stdin is piped").write_all(stdin).map_err(|source| spawn_error(args, source))?;
    let output = child.wait_with_output().map_err(|source| spawn_error(args, source))?;
    stdout(args, output)
}

fn command<T: AsRef<OsStr>>(repo: Option<&Path>, args: &[T]) -> Command {
    let mut command = Command::new("git");
    if let Some(repo) = repo {
        command.arg("-C").arg(repo);
    }
    command.args(args);
    command
}

fn stdout<T: AsRef<OsStr>>(args: &[T], output: Output) -> Result<Vec<u8>, GitError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitError::Failed {
            args: display(args),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn spawn_error<T: AsRef<OsStr>>(args: &[T], source: std::io::Error) -> GitError {
    GitError::Spawn { args: display(args), source }
}

fn display<T: AsRef<OsStr>>(args: &[T]) -> String {
    args.iter().map(|arg| arg.as_ref().to_string_lossy()).collect::<Vec<_>>().join(" ")
}
