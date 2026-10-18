use aether_core::mcp::{AETHER_MCP_IPC_SOCKET, DeferredServer, DeferredToolsClient, DeferredToolsError};
use clap::{ArgAction, Args};
use mcp_utils::client::ToolCallError;
use mcp_utils::model::{CallToolResult, Tool};
use serde_json::{Map, Value};
use std::env::var_os;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_CALL_TIMEOUT_SECONDS: u64 = 600;

#[derive(Debug, Args)]
#[command(disable_help_flag = true)]
pub struct McpArgs {
    #[arg(value_name = "SERVER")]
    server: Option<String>,

    #[arg(value_name = "TOOL", requires = "server")]
    tool: Option<String>,

    #[arg(long, action = ArgAction::SetTrue, conflicts_with_all = ["json", "timeout_seconds"])]
    help: bool,

    #[arg(long, value_name = "OBJECT", requires = "tool")]
    json: Option<String>,

    #[arg(
        long = "timeout",
        value_name = "SECONDS",
        default_value_t = DEFAULT_CALL_TIMEOUT_SECONDS,
        value_parser = clap::value_parser!(u64).range(1..),
        requires = "tool"
    )]
    timeout_seconds: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum McpCommandError {
    #[error("{0}")]
    Usage(String),
    #[error("`aether mcp` requires the inherited {AETHER_MCP_IPC_SOCKET} from an active Aether session")]
    SessionUnavailable,
    #[error("invalid {AETHER_MCP_IPC_SOCKET}: socket path must be absolute")]
    RelativeSocket,
    #[error(transparent)]
    Session(#[from] DeferredToolsError),
    #[error("MCP request timed out after {0} seconds")]
    Timeout(u64),
    #[error("MCP tool call failed: {0}")]
    Call(#[source] ToolCallError),
    #[error("deferred tool returned an error: {0}")]
    Tool(String),
    #[error("failed to read JSON from stdin: {0}")]
    Stdin(#[source] io::Error),
    #[error("failed to print JSON result: {0}")]
    Output(#[source] serde_json::Error),
}

#[derive(Debug)]
enum Request {
    ServersHelp,
    ServerHelp(String),
    ToolHelp { server: String, tool: String },
    Call { server: String, tool: String, json: Map<String, Value>, timeout_seconds: u64 },
}

impl McpCommandError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::Stdin(_) => 2,
            Self::SessionUnavailable
            | Self::RelativeSocket
            | Self::Session(_)
            | Self::Timeout(_)
            | Self::Call(_)
            | Self::Tool(_)
            | Self::Output(_) => 1,
        }
    }
}

pub async fn run(args: McpArgs) -> Result<(), McpCommandError> {
    let request = Request::try_from(args)?;
    let client = DeferredToolsClient::connect(inherited_socket()?).await?;
    match request {
        Request::ServersHelp => print_servers_help(&client.servers().await?),
        Request::ServerHelp(server) => {
            let servers = client.servers().await?;
            let deferred = servers
                .iter()
                .find(|deferred| deferred.name == server)
                .ok_or_else(|| McpCommandError::Usage(format!("unknown deferred server `{server}`")))?;
            print_server_help(deferred);
        }
        Request::ToolHelp { server, tool } => {
            let definition = client
                .tool(&server, &tool)
                .await?
                .ok_or_else(|| McpCommandError::Usage(format!("unknown deferred tool `{server} {tool}`")))?;
            print_tool_help(&server, &tool, &definition)?;
        }
        Request::Call { server, tool, json, timeout_seconds } => {
            call_tool(&client, &server, &tool, json, timeout_seconds).await?;
        }
    }
    Ok(())
}

impl TryFrom<McpArgs> for Request {
    type Error = McpCommandError;

    fn try_from(args: McpArgs) -> Result<Self, Self::Error> {
        match (args.server, args.tool, args.help) {
            (None, None, _) => Ok(Request::ServersHelp),
            (Some(server), None, true) => Ok(Request::ServerHelp(server)),
            (Some(server), Some(tool), true) => Ok(Request::ToolHelp { server, tool }),
            (Some(server), Some(tool), false) => {
                let json = if let Some(input) = args.json.as_deref() {
                    parse_json_object(Some(input))?
                } else {
                    let stdin = read_stdin()?;
                    parse_json_object(stdin.as_deref())?
                };
                Ok(Request::Call { server, tool, json, timeout_seconds: args.timeout_seconds })
            }
            (Some(_), None, false) | (None, Some(_), _) => Err(McpCommandError::Usage(
                "usage: aether mcp <server> <tool> [--json <object>] [--timeout <seconds>]".into(),
            )),
        }
    }
}

fn inherited_socket() -> Result<PathBuf, McpCommandError> {
    let path = PathBuf::from(var_os(AETHER_MCP_IPC_SOCKET).ok_or(McpCommandError::SessionUnavailable)?);
    if path.is_absolute() { Ok(path) } else { Err(McpCommandError::RelativeSocket) }
}

fn read_stdin() -> Result<Option<String>, McpCommandError> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut input = String::new();
    stdin.read_to_string(&mut input).map_err(McpCommandError::Stdin)?;
    Ok((!input.trim().is_empty()).then_some(input))
}

fn parse_json_object(input: Option<&str>) -> Result<Map<String, Value>, McpCommandError> {
    let Some(input) = input else { return Ok(Map::new()) };
    serde_json::from_str(input).map_err(|error| match error.classify() {
        serde_json::error::Category::Data => McpCommandError::Usage("tool input must be a JSON object".into()),
        _ => McpCommandError::Usage(format!("invalid JSON input: {error}")),
    })
}

async fn call_tool(
    client: &DeferredToolsClient,
    server: &str,
    tool: &str,
    json: Map<String, Value>,
    timeout_seconds: u64,
) -> Result<(), McpCommandError> {
    let call = client.call(server, tool, json, Duration::from_secs(timeout_seconds));
    let result = call.result().await.map_err(call_error)?;
    if result.is_error.unwrap_or(false) {
        return Err(McpCommandError::Tool(result_json(&result).to_string()));
    }
    println!("{}", serde_json::to_string(&result_json(&result)).map_err(McpCommandError::Output)?);
    Ok(())
}

fn call_error(error: ToolCallError) -> McpCommandError {
    match error {
        ToolCallError::TimedOut(timeout) => McpCommandError::Timeout(timeout.as_secs()),
        error => McpCommandError::Call(error),
    }
}

fn print_servers_help(servers: &[DeferredServer]) {
    println!("Discover and call deferred MCP tools through the active Aether session.\n");
    println!("Usage: aether mcp <server> --help\n");
    println!("Deferred MCP servers:");
    for server in servers {
        println!("  {:<20} {}", server.name, server.description);
    }
}

fn print_server_help(server: &DeferredServer) {
    let name = &server.name;
    println!("Deferred tools from `{name}`.\n");
    println!("Usage: aether mcp {name} <tool> --help\n");
    println!("Tools:");
    for tool in &server.tools {
        println!("  {:<20} {}", tool.name, tool.description.as_deref().unwrap_or_default());
    }
}

fn print_tool_help(server: &str, tool: &str, definition: &Tool) -> Result<(), McpCommandError> {
    println!("{}\n", definition.description.as_deref().unwrap_or_default());
    println!("Usage:");
    println!("  aether mcp {server} {tool} --json '{{...}}'");
    println!("  printf '%s' '{{...}}' | aether mcp {server} {tool}\n");
    println!("Input schema:");
    println!("{}", serde_json::to_string_pretty(definition.input_schema.as_ref()).map_err(McpCommandError::Output)?);
    Ok(())
}

fn result_json(result: &CallToolResult) -> Value {
    result.structured_content.clone().unwrap_or_else(|| serde_json::to_value(&result.content).unwrap_or(Value::Null))
}
