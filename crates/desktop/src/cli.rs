use crate::agents::CommandLine;
use clap::Parser;

pub const DEFAULT_AGENT_COMMAND: &str = "aether acp";

#[derive(Parser, Debug, Clone)]
#[command(name = "aether-desktop")]
#[command(about = "A native desktop client for running and managing many Aether agent threads")]
pub struct Cli {
    #[arg(
        long = "agent",
        help = "Command line that starts the built-in `aether` agent (speaks ACP over stdin/stdout)",
        default_value = DEFAULT_AGENT_COMMAND,
        value_parser = parse_agent_command
    )]
    pub agent: CommandLine,
}

fn parse_agent_command(command_line: &str) -> Result<CommandLine, String> {
    CommandLine::parse(command_line).map_err(|error| error.to_string())
}
