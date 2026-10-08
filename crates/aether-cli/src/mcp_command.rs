use aether_core::mcp::AETHER_MCP_IPC_SOCKET;
use clap::{ArgAction, Args};
use mcp_utils::McpError;
use mcp_utils::client::{ClientOptions, McpClient, ToolCallError, ToolCallOptions, Transport};
use mcp_utils::gateway::{LIST_SERVERS_TOOL, namespaced, split_namespaced};
use mcp_utils::model::{CallToolResult, Tool};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::env::var_os;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::time::Duration;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
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
    #[error("failed to connect to the active Aether session: {0}")]
    Connect(#[source] McpError),
    #[error("MCP request timed out after {0} seconds")]
    Timeout(u64),
    #[error("MCP request failed: {0}")]
    Request(String),
    #[error("deferred tool returned an error: {0}")]
    Tool(String),
    #[error("failed to read JSON from stdin: {0}")]
    Stdin(#[source] io::Error),
    #[error("failed to print JSON result: {0}")]
    Output(#[source] serde_json::Error),
}

#[derive(Debug)]
enum Request {
    Help(HelpLevel),
    Call { server: String, tool: String, json: Map<String, Value>, timeout_seconds: u64 },
}

#[derive(Debug)]
enum HelpLevel {
    Servers,
    Server(String),
    Tool { server: String, tool: String },
}

#[derive(Deserialize)]
struct ServerSummary {
    name: String,
    description: String,
}

impl McpCommandError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::Stdin(_) => 2,
            Self::SessionUnavailable
            | Self::RelativeSocket
            | Self::Connect(_)
            | Self::Timeout(_)
            | Self::Request(_)
            | Self::Tool(_)
            | Self::Output(_) => 1,
        }
    }
}

pub async fn run(args: McpArgs) -> Result<(), McpCommandError> {
    let request = Request::try_from(args)?;
    let transport = Transport::Unix(inherited_socket()?);
    let client =
        McpClient::connect("aether", transport, &ClientOptions::default()).await.map_err(McpCommandError::Connect)?;
    execute_request(&client, request).await
}

impl TryFrom<McpArgs> for Request {
    type Error = McpCommandError;

    fn try_from(args: McpArgs) -> Result<Self, Self::Error> {
        match (args.server, args.tool, args.help) {
            (None, None, _) => Ok(Request::Help(HelpLevel::Servers)),
            (Some(server), None, true) => Ok(Request::Help(HelpLevel::Server(server))),
            (Some(server), Some(tool), true) => Ok(Request::Help(HelpLevel::Tool { server, tool })),
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

async fn execute_request(client: &McpClient, request: Request) -> Result<(), McpCommandError> {
    match request {
        Request::Help(HelpLevel::Servers) => show_servers_help(client).await,
        Request::Help(HelpLevel::Server(server)) => show_server_help(client, &server).await,
        Request::Help(HelpLevel::Tool { server, tool }) => show_tool_help(client, &server, &tool).await,
        Request::Call { server, tool, json, timeout_seconds } => {
            call_tool(client, &server, &tool, json, timeout_seconds).await
        }
    }
}

async fn show_servers_help(client: &McpClient) -> Result<(), McpCommandError> {
    let options = ToolCallOptions::with_timeout(DISCOVERY_TIMEOUT);
    let result = client.call_tool(LIST_SERVERS_TOOL, Map::new(), options).result().await.map_err(call_error)?;
    let servers: Vec<ServerSummary> = serde_json::from_value(
        result
            .structured_content
            .ok_or_else(|| McpCommandError::Request("server discovery returned no JSON".into()))?,
    )
    .map_err(|error| McpCommandError::Request(format!("invalid server discovery response: {error}")))?;
    print_servers_help(&servers);
    Ok(())
}

async fn show_server_help(client: &McpClient, server: &str) -> Result<(), McpCommandError> {
    let tools = list_tools(client).await?;
    let tools = tools_for_server(&tools, server);
    if tools.is_empty() {
        return Err(McpCommandError::Usage(format!("unknown deferred server `{server}`")));
    }
    print_server_help(server, tools);
    Ok(())
}

async fn show_tool_help(client: &McpClient, server: &str, tool: &str) -> Result<(), McpCommandError> {
    let tools = list_tools(client).await?;
    let name = namespaced(server, tool);
    let definition = tools
        .into_iter()
        .find(|definition| definition.name == name)
        .ok_or_else(|| McpCommandError::Usage(format!("unknown deferred tool `{server} {tool}`")))?;
    print_tool_help(server, tool, &definition)
}

async fn call_tool(
    client: &McpClient,
    server: &str,
    tool: &str,
    json: Map<String, Value>,
    timeout_seconds: u64,
) -> Result<(), McpCommandError> {
    let options = ToolCallOptions::with_timeout(Duration::from_secs(timeout_seconds));
    let result = client.call_tool(&namespaced(server, tool), json, options).result().await.map_err(call_error)?;
    if result.is_error.unwrap_or(false) {
        return Err(McpCommandError::Tool(result_json(&result).to_string()));
    }
    println!("{}", serde_json::to_string(&result_json(&result)).map_err(McpCommandError::Output)?);
    Ok(())
}

async fn list_tools(client: &McpClient) -> Result<Vec<Tool>, McpCommandError> {
    tokio::time::timeout(DISCOVERY_TIMEOUT, client.list_tools())
        .await
        .map_err(|_| McpCommandError::Timeout(DISCOVERY_TIMEOUT.as_secs()))?
        .map_err(|error| McpCommandError::Request(error.to_string()))
}

fn call_error(error: ToolCallError) -> McpCommandError {
    match error {
        ToolCallError::TimedOut(timeout) => McpCommandError::Timeout(timeout.as_secs()),
        error => McpCommandError::Request(error.to_string()),
    }
}

fn tools_for_server<'a>(tools: &'a [Tool], server: &str) -> Vec<&'a Tool> {
    tools.iter().filter(|tool| split_namespaced(&tool.name).is_some_and(|(owner, _)| owner == server)).collect()
}

fn print_servers_help(servers: &[ServerSummary]) {
    println!("Discover and call deferred MCP tools through the active Aether session.\n");
    println!("Usage: aether mcp <server> --help\n");
    println!("Deferred MCP servers:");
    for server in servers {
        println!("  {:<20} {}", server.name, server.description);
    }
}

fn print_server_help(server: &str, tools: Vec<&Tool>) {
    println!("Deferred tools from `{server}`.\n");
    println!("Usage: aether mcp {server} <tool> --help\n");
    println!("Tools:");
    for tool in tools {
        let local_name = split_namespaced(&tool.name).map_or(tool.name.as_ref(), |(_, name)| name);
        println!("  {:<20} {}", local_name, tool.description.as_deref().unwrap_or_default());
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
