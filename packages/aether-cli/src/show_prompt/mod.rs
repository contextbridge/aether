mod run;

use crate::config_args::{ConfigSourceArgs, McpConfigArgs};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct PromptArgs {
    /// Working directory
    #[arg(short = 'C', long = "cwd", default_value = ".")]
    pub cwd: PathBuf,

    #[command(flatten)]
    pub config_source: ConfigSourceArgs,

    #[command(flatten)]
    pub mcp_config: McpConfigArgs,

    /// Additional system prompt
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Named agent to inspect (defaults to first user-invocable agent)
    #[arg(short = 'a', long = "agent")]
    pub agent: Option<String>,
}

pub use run::run_prompt;
