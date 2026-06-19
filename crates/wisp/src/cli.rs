use clap::Parser;

#[derive(Parser, Clone)]
#[command(name = "wisp")]
#[command(about = "A TUI for AI coding agents via the Agent Client Protocol")]
pub struct Cli {
    #[arg(
        short = 'a',
        long = "agent",
        help = "Agent subprocess command to spawn (speaks ACP over stdin/stdout)",
        default_value = "aether acp"
    )]
    pub agent: String,

    #[arg(long = "log-dir", help = "Path to log file directory (default: /tmp/wisp-logs)")]
    pub log_dir: Option<String>,
}
