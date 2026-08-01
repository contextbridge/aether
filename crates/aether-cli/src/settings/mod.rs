use crate::init::{HarnessIntegration, InitRequest, InitTargetRequest, OverwriteMode, Preset};
use clap::{Args, Subcommand};
use llm::catalog::Provider;
use std::path::PathBuf;

#[derive(Debug, Clone, Subcommand)]
pub enum SettingsCommand {
    /// Initialize Aether settings
    Init(SettingsInitArgs),
}

#[derive(Debug, Clone, Args)]
#[command(group(
    clap::ArgGroup::new("scope")
        .required(true)
        .args(["user", "project"])
))]
pub struct SettingsInitArgs {
    /// Initialize user-level settings in `$AETHER_HOME` or `~/.aether`.
    #[arg(long, conflicts_with = "project")]
    pub user: bool,

    /// Initialize project-level settings in <path>/.aether.
    #[arg(long, conflicts_with = "user")]
    pub project: bool,

    /// Project directory to initialize when --project is used.
    #[arg(long, default_value = ".", requires = "project", conflicts_with = "user")]
    pub path: PathBuf,

    /// Provider to use (e.g. `anthropic`, `codex`, `bedrock`). Skips the prompt when set.
    #[arg(long)]
    pub provider: Option<Provider>,

    /// Preset to write (`minimal` or `batteries-included`). Skips the prompt when set.
    #[arg(long)]
    pub preset: Option<Preset>,

    /// Harness conventions to auto-load (only with `batteries-included` preset).
    #[arg(long = "harness", value_enum)]
    pub harnesses: Vec<HarnessIntegration>,

    /// Overwrite an existing settings file at the selected target.
    #[arg(long)]
    pub force: bool,
}

impl From<SettingsInitArgs> for InitRequest {
    fn from(args: SettingsInitArgs) -> Self {
        let target = if args.user { InitTargetRequest::User } else { InitTargetRequest::Project { path: args.path } };
        Self {
            target,
            provider: args.provider,
            preset: args.preset,
            harnesses: args.harnesses,
            overwrite: if args.force { OverwriteMode::OverwriteExisting } else { OverwriteMode::PreserveExisting },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: SettingsCommand,
    }

    fn parse(args: &[&str]) -> Result<TestCli, clap::Error> {
        TestCli::try_parse_from(std::iter::once("test").chain(args.iter().copied()))
    }

    #[test]
    fn accepts_user_scope() {
        let cli = parse(&["init", "--user"]).unwrap();
        let SettingsCommand::Init(args) = cli.command;
        assert!(args.user);
        assert!(!args.project);
    }

    #[test]
    fn accepts_project_scope_and_path() {
        let cli = parse(&["init", "--project", "--path", "/tmp/repo"]).unwrap();
        let SettingsCommand::Init(args) = cli.command;
        assert!(args.project);
        assert_eq!(args.path, PathBuf::from("/tmp/repo"));
    }

    #[test]
    fn accepts_provider_preset_and_force() {
        let cli = parse(&["init", "--user", "--provider", "codex", "--preset", "minimal", "--force"]).unwrap();
        let SettingsCommand::Init(args) = cli.command;
        assert_eq!(args.provider, Some(Provider::Codex));
        assert_eq!(args.preset, Some(Preset::Minimal));
        assert!(args.force);
    }

    #[test]
    fn rejects_missing_scope() {
        assert!(parse(&["init"]).is_err());
    }

    #[test]
    fn rejects_both_scopes() {
        assert!(parse(&["init", "--user", "--project"]).is_err());
    }

    #[test]
    fn rejects_user_path() {
        assert!(parse(&["init", "--user", "--path", "/tmp/repo"]).is_err());
    }

    #[test]
    fn accepts_repeated_harness_flags() {
        let cli =
            parse(&["init", "--user", "--preset", "batteries-included", "--harness", "claude", "--harness", "agents"])
                .unwrap();
        let SettingsCommand::Init(args) = cli.command;
        assert_eq!(args.harnesses, vec![HarnessIntegration::Claude, HarnessIntegration::Agents]);
    }
}
