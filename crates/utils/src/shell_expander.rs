use futures::future::join_all;
use regex::Regex;
use std::env;
use std::path::Path;
use std::sync::LazyLock;
use tokio::process::Command;

/// Expands `` !`command` `` markers in `content` by running each command via
/// `$SHELL -c` (fallback `sh`) from `cwd` and substituting the trimmed stdout.
/// Returns `content` unchanged if no markers are present.
pub async fn expand_shell(content: &str, cwd: &Path) -> String {
    if !SHELL_MARKER.is_match(content) {
        return content.to_string();
    }

    let spans: Vec<(usize, usize, &str)> = SHELL_MARKER
        .captures_iter(content)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            let cmd = captures.get(1)?;
            Some((whole.start(), whole.end(), cmd.as_str()))
        })
        .collect();

    let outputs = join_all(spans.iter().map(|(_, _, cmd)| run(cmd, cwd))).await;
    let mut out = String::with_capacity(content.len());
    let mut last = 0;

    for ((start, end, _), result) in spans.iter().zip(outputs) {
        out.push_str(&content[last..*start]);
        match result {
            Ok(output) => out.push_str(&output),
            Err(err) => tracing::warn!("{err}"),
        }
        last = *end;
    }

    out.push_str(&content[last..]);
    out
}

#[derive(Debug, thiserror::Error)]
enum ShellExpansionError {
    #[error("Failed to spawn {shell} for `{cmd}`: {error}")]
    Spawn { shell: String, cmd: String, error: String },
    #[error("Shell interpolation `{cmd}` failed with {status}: {stderr}")]
    NonZeroExit { cmd: String, status: String, stderr: String },
}

static SHELL_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!`([^`\n]+)`").expect("valid regex"));

async fn run(cmd: &str, cwd: &Path) -> Result<String, ShellExpansionError> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let output =
        Command::new(&shell).arg("-c").arg(cmd).current_dir(cwd).output().await.map_err(|e| {
            ShellExpansionError::Spawn { shell: shell.clone(), cmd: cmd.to_string(), error: e.to_string() }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ShellExpansionError::NonZeroExit {
            cmd: cmd.to_string(),
            status: output.status.to_string(),
            stderr: stderr.trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_op_without_marker() {
        let content = "Just some plain content with no directives";
        let cwd = std::env::current_dir().unwrap();
        let result = expand_shell(content, &cwd).await;
        assert_eq!(result, content);
    }

    #[tokio::test]
    async fn runs_shell_command() {
        let cwd = std::env::current_dir().unwrap();
        let result = expand_shell("branch: !`echo main`", &cwd).await;
        assert_eq!(result, "branch: main");
    }

    #[tokio::test]
    async fn runs_command_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sentinel.txt"), "").unwrap();

        let result = expand_shell("files: !`ls`", dir.path()).await;
        assert!(result.contains("sentinel.txt"), "expected sentinel.txt in output: {result}");
    }

    #[tokio::test]
    async fn handles_multiple_commands() {
        let cwd = std::env::current_dir().unwrap();
        let result = expand_shell("a=!`echo one`, b=!`echo two`", &cwd).await;
        assert_eq!(result, "a=one, b=two");
    }

    #[tokio::test]
    async fn failed_command_substitutes_empty_string() {
        let cwd = std::env::current_dir().unwrap();
        let result = expand_shell("before !`exit 1` after", &cwd).await;
        assert_eq!(result, "before  after");
    }

    #[tokio::test]
    async fn trims_trailing_whitespace() {
        let cwd = std::env::current_dir().unwrap();
        let result = expand_shell("!`printf 'hi\\n\\n'`", &cwd).await;
        assert_eq!(result, "hi");
    }
}
