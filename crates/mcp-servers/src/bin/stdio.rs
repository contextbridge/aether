use clap::Parser;
use mcp_servers::workspace_paths::{current_dir, resolve_path};
use mcp_servers::{CodingMcp, CodingMcpArgs, ReviewMcp, SkillsMcp, SubAgentsMcp, TasksMcp};
use mcp_utils::server::McpServer;

#[derive(Parser)]
#[command(name = "mcp-servers-stdio", about = "Run an MCP server over stdio")]
struct Cli {
    /// Which server to run: coding, skills, tasks, subagents, review
    #[arg(long)]
    server: String,

    /// Arguments forwarded to the selected server (e.g. --root-dir /path)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum StdioError {
    #[error("Unknown server: '{0}'. Available: coding, skills, tasks, subagents, review")]
    UnknownServer(String),
    #[error("{0}")]
    ServerArgs(#[from] mcp_servers::error::ServerInitError),
    #[error("{0}")]
    Serve(#[from] mcp_utils::McpError),
}

#[tokio::main]
async fn main() -> Result<(), StdioError> {
    let cli = Cli::parse();

    let server = match cli.server.as_str() {
        "coding" => {
            let CodingMcpArgs { root_dir, rules_dirs, permission_mode, disable_lsp } =
                CodingMcpArgs::from_args(cli.args)?;
            let root_dir = root_dir.unwrap_or_else(current_dir);
            let rules_dirs = rules_dirs.into_iter().map(|path| resolve_path(&root_dir, path)).collect();
            let server = CodingMcp::new()
                .with_root_dir(root_dir.clone())
                .with_rules_dirs(rules_dirs)
                .with_permission_mode(permission_mode);
            McpServer::new(if disable_lsp { server } else { server.with_lsp(root_dir) })
        }
        "skills" => McpServer::new(SkillsMcp::from_args(cli.args)?),
        "tasks" => McpServer::new(TasksMcp::from_args(cli.args)?),
        "subagents" => McpServer::new(SubAgentsMcp::standalone_from_args(cli.args)?),
        "review" => McpServer::new(ReviewMcp::from_args(cli.args)?),
        other => return Err(StdioError::UnknownServer(other.to_string())),
    };
    Ok(server.serve_stdio().await?)
}
