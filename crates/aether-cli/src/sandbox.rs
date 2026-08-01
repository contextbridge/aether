use std::env;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use llm::LlmModel;
use thiserror::Error;

/// Credentials and settings not covered by `LlmModel::ALL_REQUIRED_ENV_VARS`.
///
/// Bedrock resolves credentials through the AWS chain rather than a single
/// required env var, so its variables have to be listed explicitly or a
/// sandboxed run cannot authenticate at all.
const EXTRA_FORWARDED_KEYS: &[&str] = &[
    "OLLAMA_HOST",
    "AWS_ACCESS_KEY_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_DEFAULT_REGION",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
];

const AWS_PATH_KEYS: &[&str] = &["AWS_CONFIG_FILE", "AWS_SHARED_CREDENTIALS_FILE", "AWS_WEB_IDENTITY_TOKEN_FILE"];
const AWS_SHARED_PATHS: &[&str] = &["config", "credentials", "sso/cache", "login/cache"];
const AETHER_ENV_PREFIX: &str = "AETHER_";

/// Host paths made available inside the container, and the env values that point at them.
struct AwsBindings {
    mounts: Vec<BindMount>,
    env_overrides: Vec<(String, String)>,
}

struct BindMount {
    host: PathBuf,
    container: PathBuf,
}

/// Whether the sandboxed process runs attached to an interactive terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalMode {
    Interactive,
    NonInteractive,
}

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("Docker is not installed or not in PATH")]
    DockerNotFound,
    #[error("Docker daemon is not running: {0}")]
    DockerNotRunning(String),
    #[error(
        "Sandbox image '{0}' not found. Build it with:\n\
             cargo build -p aether-agent-cli --bin aether\n\
             docker build -t {0} -f crates/internal-evals/examples/Dockerfile ."
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
    let home = dirs::home_dir().ok_or(SandboxError::HomeNotResolvable)?;
    let aether_home = resolve_aether_home(&home);
    let args: Vec<String> = env::args().collect();
    let inner_args = filter_sandbox_arg(&args);
    let mut env_vars = select_forwarded_vars(env::vars());
    let aws = aws_bindings(&home, &env_vars);
    apply_overrides(&mut env_vars, aws.env_overrides);

    let terminal = if io::stdin().is_terminal() { TerminalMode::Interactive } else { TerminalMode::NonInteractive };
    let docker_args = build_docker_args(image, &cwd, &aether_home, &aws.mounts, &env_vars, &inner_args, terminal);

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

fn resolve_aether_home(home: &Path) -> PathBuf {
    env::var("AETHER_HOME").map_or_else(|_| home.join(".aether"), PathBuf::from)
}

fn aws_bindings(home: &Path, env_vars: &[(String, String)]) -> AwsBindings {
    let mut mounts = Vec::new();
    let mut env_overrides = Vec::new();

    let aws_home = home.join(".aws");
    for relative in AWS_SHARED_PATHS {
        let host = aws_home.join(relative);
        if host.exists() {
            mounts.push(BindMount { host, container: Path::new("/root/.aws").join(relative) });
        }
    }

    for (key, value) in env_vars.iter().filter(|(key, _)| AWS_PATH_KEYS.contains(&key.as_str())) {
        let host = PathBuf::from(value);
        if !host.is_file() {
            continue;
        }
        let container = PathBuf::from(format!("/run/aether-aws/{key}"));
        env_overrides.push((key.clone(), container.to_string_lossy().into_owned()));
        mounts.push(BindMount { host, container });
    }

    AwsBindings { mounts, env_overrides }
}

fn apply_overrides(env_vars: &mut [(String, String)], overrides: Vec<(String, String)>) {
    for (key, replacement) in overrides {
        if let Some((_, value)) = env_vars.iter_mut().find(|(existing, _)| *existing == key) {
            *value = replacement;
        }
    }
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
            || AWS_PATH_KEYS.contains(&key.as_str())
            || key.starts_with(AETHER_ENV_PREFIX)
    })
    .collect()
}

fn build_docker_args(
    image: &str,
    cwd: &Path,
    aether_home: &Path,
    mounts: &[BindMount],
    env_vars: &[(String, String)],
    inner_args: &[String],
    terminal: TerminalMode,
) -> Vec<String> {
    let mut args = vec!["run".to_string(), "--rm".to_string(), "-i".to_string()];
    if terminal == TerminalMode::Interactive {
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

    for mount in mounts {
        args.push("-v".to_string());
        args.push(format!("{}:{}:ro", mount.host.display(), mount.container.display()));
    }

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

        let args = build_docker_args(
            "test-image:latest",
            cwd,
            aether_home,
            &[],
            &env_vars,
            &inner_args,
            TerminalMode::NonInteractive,
        );

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
            &[],
            &["aether".to_string(), "headless".to_string()],
            TerminalMode::NonInteractive,
        );

        assert!(args.contains(&"my-go-sandbox:v2".to_string()));
        assert!(!args.contains(&"test-image:latest".to_string()));
    }

    #[test]
    fn build_docker_args_adds_tty_flag_when_requested() {
        let cwd = Path::new("/tmp");
        let aether_home = Path::new("/home/user/.aether");
        let args = build_docker_args(
            "test-image",
            cwd,
            aether_home,
            &[],
            &[],
            &["aether".to_string()],
            TerminalMode::Interactive,
        );

        assert!(args.contains(&"-t".to_string()));
        assert!(args.contains(&"-i".to_string()));
    }

    #[test]
    fn build_docker_args_skips_binary_name_only() {
        let cwd = Path::new("/tmp");
        let aether_home = Path::new("/home/user/.aether");
        let args = build_docker_args(
            "test-image:latest",
            cwd,
            aether_home,
            &[],
            &[],
            &["aether".to_string()],
            TerminalMode::NonInteractive,
        );

        // Only the binary name — nothing after image
        assert_eq!(args.last().unwrap(), "test-image:latest");
    }

    /// A `~/.aws` holding one of everything: what the SDK reads, plus the AWS
    /// CLI's own credential cache.
    fn fake_aws_home(home: &Path) {
        let aws = home.join(".aws");
        std::fs::create_dir_all(aws.join("sso/cache")).unwrap();
        std::fs::create_dir_all(aws.join("cli/cache")).unwrap();
        std::fs::write(aws.join("config"), "[default]\n").unwrap();
        std::fs::write(aws.join("credentials"), "[default]\n").unwrap();
        std::fs::write(aws.join("sso/cache/token.json"), "{}").unwrap();
        std::fs::write(aws.join("cli/cache/assumed-role.json"), "{}").unwrap();
    }

    #[test]
    fn aws_bindings_mount_the_profile_and_rewrite_custom_files() {
        let home = tempfile::tempdir().unwrap();
        fake_aws_home(home.path());
        let token = home.path().join("web-identity-token");
        std::fs::write(&token, "token").unwrap();
        let mut env_vars = vec![("AWS_WEB_IDENTITY_TOKEN_FILE".to_string(), token.to_string_lossy().into_owned())];

        let aws = aws_bindings(home.path(), &env_vars);
        apply_overrides(&mut env_vars, aws.env_overrides);

        assert!(aws.mounts.iter().any(|mount| mount.container == Path::new("/root/.aws/config")));
        assert!(aws.mounts.iter().any(|mount| mount.container == Path::new("/root/.aws/sso/cache")));
        assert!(aws.mounts.iter().any(|mount| mount.host == token));
        assert_eq!(env_vars[0].1, "/run/aether-aws/AWS_WEB_IDENTITY_TOKEN_FILE");

        let args = build_docker_args(
            "test-image",
            Path::new("/workspace"),
            &home.path().join(".aether"),
            &aws.mounts,
            &env_vars,
            &["aether".to_string()],
            TerminalMode::NonInteractive,
        );
        assert!(args.contains(&format!("{}:/root/.aws/config:ro", home.path().join(".aws/config").display())));
        assert!(args.contains(&"AWS_WEB_IDENTITY_TOKEN_FILE=/run/aether-aws/AWS_WEB_IDENTITY_TOKEN_FILE".to_string()));
    }

    #[test]
    fn aws_bindings_never_share_the_aws_cli_credential_cache() {
        let home = tempfile::tempdir().unwrap();
        fake_aws_home(home.path());

        let aws = aws_bindings(home.path(), &[]);

        assert!(
            !aws.mounts.iter().any(|mount| mount.host.to_string_lossy().contains("/.aws/cli")),
            "the AWS CLI's cache holds credentials the SDK never reads: {:?}",
            aws.mounts.iter().map(|mount| mount.host.display().to_string()).collect::<Vec<_>>()
        );
        assert!(
            !aws.mounts.iter().any(|mount| mount.container == Path::new("/root/.aws")),
            "sharing the whole directory would expose everything under it"
        );
    }

    #[test]
    fn aws_bindings_mount_only_the_paths_that_exist() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".aws")).unwrap();
        std::fs::write(home.path().join(".aws/config"), "[default]\n").unwrap();

        let aws = aws_bindings(home.path(), &[]);

        assert_eq!(aws.mounts.len(), 1);
        assert_eq!(aws.mounts[0].container, Path::new("/root/.aws/config"));
    }

    #[test]
    fn aws_bindings_skip_env_paths_that_do_not_exist_on_the_host() {
        let home = tempfile::tempdir().unwrap();
        let env_vars = vec![("AWS_CONFIG_FILE".to_string(), "/nowhere/config".to_string())];

        let aws = aws_bindings(home.path(), &env_vars);

        assert!(aws.mounts.is_empty(), "nothing to mount when neither ~/.aws nor the named file exists");
        assert!(aws.env_overrides.is_empty(), "a missing host file must keep its original value");
    }

    #[test]
    fn forwarded_vars_include_the_aws_path_variables() {
        let vars = [
            ("AWS_SHARED_CREDENTIALS_FILE".to_string(), "/home/user/creds".to_string()),
            ("UNRELATED".to_string(), "x".to_string()),
        ];

        let forwarded = select_forwarded_vars(vars.into_iter());

        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].0, "AWS_SHARED_CREDENTIALS_FILE");
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
