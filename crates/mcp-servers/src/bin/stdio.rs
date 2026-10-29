use aether_core::core::AgentDeps;
use aether_core::mcp::RuntimeServices;
use clap::Parser;
use mcp_servers::workspace_paths::current_dir;
use mcp_servers::{Builtin, SubAgentsMcp, SubAgentsMcpArgs};
use mcp_utils::server::{McpServer, ServeError};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "mcp-servers-stdio", about = "Run an MCP server over stdio")]
struct Cli {
    #[arg(long, value_enum)]
    server: Builtin,

    /// Arguments forwarded to the selected server (e.g. --root-dir /path)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum StdioError {
    #[error(transparent)]
    Settings(#[from] aether_project::SettingsError),
    #[error(transparent)]
    Serve(#[from] ServeError),
}

fn main() -> Result<ExitCode, StdioError> {
    #[cfg(feature = "coding")]
    if let Some(code) = clankerdiff_lspd::run_if_requested() {
        return Ok(code);
    }
    let Cli { server, args } = Cli::parse();
    serve(server, args).map(|()| ExitCode::SUCCESS)
}

#[tokio::main]
async fn serve(server: Builtin, args: Vec<String>) -> Result<(), StdioError> {
    let services =
        RuntimeServices { root_dir: current_dir(), agent_deps: AgentDeps::default(), deferred_tools_socket: None };
    let server: McpServer = match server {
        Builtin::Subagents => {
            let args = SubAgentsMcpArgs::try_parse_from(args).unwrap_or_else(|error| error.exit());
            SubAgentsMcp::standalone(args, &services.root_dir)?.into()
        }
        builtin => builtin.build(args, &services).unwrap_or_else(|error| error.exit()),
    };
    Ok(server.serve_stdio().await?)
}
