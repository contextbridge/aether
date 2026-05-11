use clap::Parser;
use mcp_servers::{CodingMcp, CodingMcpArgs, PlanMcp, SkillsMcp, SubAgentsMcp, SurveyMcp, TasksMcp};
use mcp_utils::ServiceExt;
use rmcp::ServerHandler;
use rmcp::transport::io::stdio;

#[derive(Parser)]
#[command(name = "mcp-servers-stdio", about = "Run an MCP server over stdio")]
struct Cli {
    /// Which server to run: coding, skills, tasks, subagents, survey, plan
    #[arg(long)]
    server: String,

    /// Arguments forwarded to the selected server (e.g. --root-dir /path)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug)]
enum StdioError {
    UnknownServer(String),
    ServerArgs(String),
    Serve(String),
    Join(tokio::task::JoinError),
}

impl std::fmt::Display for StdioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StdioError::UnknownServer(name) => {
                write!(f, "Unknown server: '{name}'. Available: coding, skills, tasks, subagents, survey, plan")
            }
            StdioError::ServerArgs(msg) => write!(f, "{msg}"),
            StdioError::Serve(msg) => write!(f, "Failed to start server: {msg}"),
            StdioError::Join(e) => write!(f, "Server task failed: {e}"),
        }
    }
}

async fn serve_stdio(server: impl ServerHandler) -> Result<(), StdioError> {
    let running = server.serve(stdio()).await.map_err(|e| StdioError::Serve(e.to_string()))?;
    running.waiting().await.map_err(StdioError::Join)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), StdioError> {
    let cli = Cli::parse();

    match cli.server.as_str() {
        "coding" => {
            let CodingMcpArgs { root_dir, rules_dirs, permission_mode, disable_lsp } =
                CodingMcpArgs::from_args(cli.args).map_err(StdioError::ServerArgs)?;
            let server = CodingMcp::new().with_rules_dirs(rules_dirs).with_permission_mode(permission_mode);
            let server = match root_dir {
                Some(root_dir) if !disable_lsp => server.with_root_dir(root_dir.clone()).with_lsp(root_dir),
                Some(root_dir) => server.with_root_dir(root_dir),
                None => server,
            };
            serve_stdio(server).await
        }
        "skills" => {
            let server = SkillsMcp::from_args(cli.args).map_err(StdioError::ServerArgs)?;
            serve_stdio(server).await
        }
        "tasks" => {
            let server = TasksMcp::from_args(cli.args).map_err(StdioError::ServerArgs)?;
            serve_stdio(server).await
        }
        "subagents" => {
            let server = SubAgentsMcp::from_args(cli.args).map_err(StdioError::ServerArgs)?;
            serve_stdio(server).await
        }
        "survey" => {
            let server = SurveyMcp::from_args(cli.args).map_err(StdioError::ServerArgs)?;
            serve_stdio(server).await
        }
        "plan" => {
            let server = PlanMcp::from_args(cli.args).map_err(StdioError::ServerArgs)?;
            serve_stdio(server).await
        }
        other => Err(StdioError::UnknownServer(other.to_string())),
    }
}
