use std::env;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use llm::LlmModel;
use thiserror::Error;

const EXTRA_FORWARDED_KEYS: &[&str] = &["OLLAMA_HOST"];

const AETHER_ENV_PREFIX: &str = "AETHER_";

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("Docker is not installed or not in PATH")]
    DockerNotFound,
    #[error("Docker daemon is not running: {0}")]
    DockerNotRunning(String),
    #[error(
        "Sandbox image '{0}' not found. Build it with:\n\
             cargo build --release -p aether-agent-cli\n\
             cp target/release/aether docker/\n\
             docker build -t {0} -f docker/Dockerfile.sandbox docker/"
    )]
    ImageNotFound(String),
    #[error("Failed to exec docker: {0}")]
    ExecFailed(#[from] io::Error),
    #[error("Could not determine home directory")]
    HomeNotResolvable,
}

/// Entry point called from `main()` when `--sandbox-image` is present.
pub fn exec_in_container(image: &str) -> ExitCode {
    match try_exec_in_container(image) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Sandbox error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn try_exec_in_container(image: &str) -> Result<ExitCode, SandboxError> {
    check_docker()?;
    check_image(image)?;

    let cwd = env::current_dir().map_err(SandboxError::ExecFailed)?;
    let aether_home = resolve_aether_home()?;
    let args: Vec<String> = env::args().collect();
    let inner_args = filter_sandbox_arg(&args);
    let env_vars = select_forwarded_vars(env::vars());

    let tty = io::stdin().is_terminal();
    let docker_args = build_docker_args(image, &cwd, &aether_home, &env_vars, &inner_args, tty);

    exec_docker(&docker_args)
}

fn check_docker() -> Result<(), SandboxError> {
    let output = Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|_| SandboxError::DockerNotFound)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(SandboxError::DockerNotRunning(stderr));
    }

    Ok(())
}

fn check_image(image: &str) -> Result<(), SandboxError> {
    let output = Command::new("docker")
        .args(["image", "inspect", image])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|_| SandboxError::DockerNotFound)?;

    if !output.status.success() {
        return Err(SandboxError::ImageNotFound(image.to_string()));
    }

    Ok(())
}

fn resolve_aether_home() -> Result<PathBuf, SandboxError> {
    if let Ok(val) = env::var("AETHER_HOME") {
        return Ok(PathBuf::from(val));
    }
    let home = dirs::home_dir().ok_or(SandboxError::HomeNotResolvable)?;
    Ok(home.join(".aether"))
}

fn filter_sandbox_arg(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--sandbox-image" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--sandbox-image=") {
            continue;
        }
        result.push(arg.clone());
    }
    result
}

fn select_forwarded_vars(vars: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    vars.filter(|(key, _)| {
        LlmModel::ALL_REQUIRED_ENV_VARS.contains(&key.as_str())
            || EXTRA_FORWARDED_KEYS.contains(&key.as_str())
            || key.starts_with(AETHER_ENV_PREFIX)
    })
    .collect()
}

fn build_docker_args(
    image: &str,
    cwd: &Path,
    aether_home: &Path,
    env_vars: &[(String, String)],
    inner_args: &[String],
    tty: bool,
) -> Vec<String> {
    let mut args = vec!["run".to_string(), "--rm".to_string(), "-i".to_string()];
    if tty {
        args.push("-t".to_string());
    }
    args.extend(
        [
            "--network",
            "host",
            "-w",
            "/workspace",
            "-v",
            &format!("{}:/workspace", cwd.display()),
            "-v",
            &format!("{}:/root/.aether", aether_home.display()),
            "-e",
            "AETHER_HOME=/root/.aether",
            "-e",
            "AETHER_INSIDE_SANDBOX=1",
        ]
        .iter()
        .map(ToString::to_string),
    );

    for (key, value) in env_vars {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }

    args.push(image.to_string());

    // Skip the binary name (first element) — the ENTRYPOINT already provides it
    if inner_args.len() > 1 {
        args.extend(inner_args[1..].iter().cloned());
    }

    args
}

#[cfg(unix)]
fn exec_docker(args: &[String]) -> Result<ExitCode, SandboxError> {
    use std::os::unix::process::CommandExt;

    let err = Command::new("docker").args(args).exec();
    Err(SandboxError::ExecFailed(err))
}

#[cfg(not(unix))]
fn exec_docker(args: &[String]) -> Result<ExitCode, SandboxError> {
    let status = Command::new("docker").args(args).status().map_err(SandboxError::ExecFailed)?;

    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_sandbox_arg_strips_separate_value() {
        let args = vec![
            "aether".to_string(),
            "--sandbox-image".to_string(),
            "my-image:latest".to_string(),
            "headless".to_string(),
            "-m".to_string(),
            "gpt-4".to_string(),
        ];
        let filtered = filter_sandbox_arg(&args);
        assert_eq!(filtered, vec!["aether", "headless", "-m", "gpt-4"]);
    }

    #[test]
    fn filter_sandbox_arg_strips_equals_form() {
        let args = vec!["aether".to_string(), "--sandbox-image=my-image:latest".to_string(), "headless".to_string()];
        let filtered = filter_sandbox_arg(&args);
        assert_eq!(filtered, vec!["aether", "headless"]);
    }

    #[test]
    fn filter_sandbox_arg_noop_when_absent() {
        let args = vec!["aether".to_string(), "headless".to_string(), "-m".to_string()];
        let filtered = filter_sandbox_arg(&args);
        assert_eq!(filtered, args);
    }

    #[test]
    fn filter_sandbox_arg_middle_position() {
        let args = vec![
            "aether".to_string(),
            "headless".to_string(),
            "--sandbox-image".to_string(),
            "custom:v2".to_string(),
            "-m".to_string(),
        ];
        let filtered = filter_sandbox_arg(&args);
        assert_eq!(filtered, vec!["aether", "headless", "-m"]);
    }

    #[test]
    fn select_forwarded_vars_includes_generated_provider_keys() {
        let vars = vec![
            ("ANTHROPIC_API_KEY".to_string(), "sk-123".to_string()),
            ("OPENROUTER_API_KEY".to_string(), "or-456".to_string()),
            ("ZAI_API_KEY".to_string(), "zai-789".to_string()),
            ("DEEPSEEK_API_KEY".to_string(), "ds-000".to_string()),
            ("HOME".to_string(), "/root".to_string()),
        ];
        let forwarded = select_forwarded_vars(vars.into_iter());
        assert_eq!(forwarded.len(), 4);
        assert!(forwarded.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
        assert!(forwarded.iter().any(|(k, _)| k == "OPENROUTER_API_KEY"));
        assert!(forwarded.iter().any(|(k, _)| k == "ZAI_API_KEY"));
        assert!(forwarded.iter().any(|(k, _)| k == "DEEPSEEK_API_KEY"));
    }

    #[test]
    fn select_forwarded_vars_includes_extra_keys() {
        let vars = vec![
            ("OLLAMA_HOST".to_string(), "http://localhost:11434".to_string()),
            ("HOME".to_string(), "/root".to_string()),
        ];
        let forwarded = select_forwarded_vars(vars.into_iter());
        assert_eq!(forwarded.len(), 1);
        assert!(forwarded.iter().any(|(k, _)| k == "OLLAMA_HOST"));
    }

    #[test]
    fn select_forwarded_vars_includes_aether_prefix() {
        let vars = vec![
            ("AETHER_DEBUG".to_string(), "1".to_string()),
            ("AETHER_LOG_LEVEL".to_string(), "trace".to_string()),
            ("SOMETHING_ELSE".to_string(), "nope".to_string()),
        ];
        let forwarded = select_forwarded_vars(vars.into_iter());
        assert_eq!(forwarded.len(), 2);
        assert!(forwarded.iter().any(|(k, _)| k == "AETHER_DEBUG"));
        assert!(forwarded.iter().any(|(k, _)| k == "AETHER_LOG_LEVEL"));
    }

    #[test]
    fn select_forwarded_vars_excludes_unknown() {
        let vars = vec![("HOME".to_string(), "/root".to_string()), ("EDITOR".to_string(), "vim".to_string())];
        let forwarded = select_forwarded_vars(vars.into_iter());
        assert!(forwarded.is_empty());
    }

    #[test]
    fn all_required_env_vars_stays_in_sync() {
        // If a new provider is added to codegen, this test reminds us it's auto-forwarded
        assert!(LlmModel::ALL_REQUIRED_ENV_VARS.contains(&"ANTHROPIC_API_KEY"));
        assert!(LlmModel::ALL_REQUIRED_ENV_VARS.contains(&"ZAI_API_KEY"));
        assert!(LlmModel::ALL_REQUIRED_ENV_VARS.contains(&"DEEPSEEK_API_KEY"));
    }

    #[test]
    fn build_docker_args_contains_expected_flags() {
        let cwd = Path::new("/home/user/project");
        let aether_home = Path::new("/home/user/.aether");
        let env_vars = vec![("ANTHROPIC_API_KEY".to_string(), "sk-123".to_string())];
        let inner_args = vec!["aether".to_string(), "headless".to_string(), "-m".to_string(), "gpt-4".to_string()];

        let args = build_docker_args("test-image:latest", cwd, aether_home, &env_vars, &inner_args, false);

        assert!(args.contains(&"run".to_string()));
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(!args.contains(&"-t".to_string()));
        assert!(args.contains(&"--network".to_string()));
        assert!(args.contains(&"host".to_string()));
        assert!(args.contains(&"/workspace".to_string()));
        assert!(args.contains(&format!("{}:/workspace", cwd.display())));
        assert!(args.contains(&format!("{}:/root/.aether", aether_home.display())));
        assert!(args.contains(&"AETHER_HOME=/root/.aether".to_string()));
        assert!(args.contains(&"AETHER_INSIDE_SANDBOX=1".to_string()));
        assert!(args.contains(&"ANTHROPIC_API_KEY=sk-123".to_string()));
        assert!(args.contains(&"test-image:latest".to_string()));
        // Inner args skip the binary name
        assert!(args.contains(&"headless".to_string()));
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"gpt-4".to_string()));
        // Binary name must NOT appear after the image
        let image_pos = args.iter().position(|a| a == "test-image:latest").unwrap();
        assert!(!args[image_pos..].contains(&"aether".to_string()));
    }

    #[test]
    fn build_docker_args_uses_custom_image() {
        let cwd = Path::new("/tmp");
        let aether_home = Path::new("/home/user/.aether");
        let args = build_docker_args(
            "my-go-sandbox:v2",
            cwd,
            aether_home,
            &[],
            &["aether".to_string(), "headless".to_string()],
            false,
        );

        assert!(args.contains(&"my-go-sandbox:v2".to_string()));
        assert!(!args.contains(&"test-image:latest".to_string()));
    }

    #[test]
    fn build_docker_args_adds_tty_flag_when_requested() {
        let cwd = Path::new("/tmp");
        let aether_home = Path::new("/home/user/.aether");
        let args = build_docker_args("test-image", cwd, aether_home, &[], &["aether".to_string()], true);

        assert!(args.contains(&"-t".to_string()));
        assert!(args.contains(&"-i".to_string()));
    }

    #[test]
    fn build_docker_args_skips_binary_name_only() {
        let cwd = Path::new("/tmp");
        let aether_home = Path::new("/home/user/.aether");
        let args = build_docker_args("test-image:latest", cwd, aether_home, &[], &["aether".to_string()], false);

        // Only the binary name — nothing after image
        assert_eq!(args.last().unwrap(), "test-image:latest");
    }

    #[test]
    fn sandbox_error_display_messages() {
        assert_eq!(SandboxError::DockerNotFound.to_string(), "Docker is not installed or not in PATH");

        assert!(SandboxError::DockerNotRunning("connection refused".into()).to_string().contains("connection refused"));

        let img_err = SandboxError::ImageNotFound("aether-sandbox:latest".into());
        assert!(img_err.to_string().contains("aether-sandbox:latest"));
        assert!(img_err.to_string().contains("cargo build"));

        assert!(SandboxError::HomeNotResolvable.to_string().contains("home directory"));

        let io_err = io::Error::new(io::ErrorKind::NotFound, "not found");
        assert!(SandboxError::ExecFailed(io_err).to_string().contains("not found"));
    }
}
