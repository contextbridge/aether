use llm::ToolDefinition;
use mcp_utils::ServiceExt;
use mcp_utils::client::split_on_server_name;
use mcp_utils::tool_gateway::{AETHER_MCP_IPC_SOCKET, LIST_SERVERS_TOOL, ServerDescription, UnixSocketPath};
use rmcp::{
    RoleClient,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
};
use serde_json::{Map, Value};
use std::env::var_os;
use std::fmt::Write as _;
use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::timeout;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const EXECUTION_OVERHEAD: Duration = Duration::from_secs(5);

#[derive(Debug, clap::Args)]
#[command(disable_help_flag = true)]
pub struct McpArgs {
    /// Tool execution timeout in seconds.
    #[arg(long, default_value_t = 600)]
    pub timeout: u64,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpCliError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Runtime(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl McpCliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) => ExitCode::from(2),
            Self::Runtime(_) | Self::Io(_) => ExitCode::FAILURE,
        }
    }
}

pub async fn run_mcp(args: McpArgs) -> Result<(), McpCliError> {
    let client = inherited_client().await?;
    let stdin = read_stdin_if_piped()?;
    let timeout = Duration::from_secs(args.timeout);
    let command: Vec<&str> = args.command.iter().map(String::as_str).collect();
    let output = match command.as_slice() {
        [] | ["--help"] => {
            let response = request(&client, CallToolRequestParams::new(LIST_SERVERS_TOOL), DISCOVERY_TIMEOUT).await?;
            let servers: Vec<ServerDescription> =
                serde_json::from_value(tool_result_value(response)?).map_err(runtime_error)?;
            let mut help = String::from("Usage: aether mcp <server> <tool> [arguments]\n\nDeferred MCP servers:\n");
            for server in servers {
                let _ = writeln!(help, "  {:20} {}", server.name, one_line(&server.description));
            }
            help.trim_end().to_string()
        }
        [server] | [server, "--help", ..] => {
            let tools = list_tools(&client).await?.into_iter().filter(|tool| tool.server.as_deref() == Some(*server));
            let mut help = format!("Usage: aether mcp {server} <tool> [arguments]\n\nDeferred tools:\n");
            for tool in tools {
                let _ = writeln!(help, "  {:20} {}", tool.name, one_line(&tool.description));
            }
            help.trim_end().to_string()
        }
        [server, tool, "--help", ..] => {
            let definition = list_tools(&client)
                .await?
                .into_iter()
                .find(|definition| definition.server.as_deref() == Some(*server) && definition.name == *tool)
                .ok_or_else(|| McpCliError::Runtime(format!("tool '{tool}' was not found on server '{server}'")))?;
            describe_tool(&definition)
        }
        [server, tool, arguments @ ..] => {
            let arguments = parse_arguments(arguments, &stdin)?;
            let execute = CallToolRequestParams::new(format!("{server}__{tool}")).with_arguments(arguments);
            let response = request(&client, execute, timeout.saturating_add(EXECUTION_OVERHEAD)).await?;
            serde_json::to_string(&tool_result_value(response)?).map_err(runtime_error)?
        }
    };
    println!("{output}");
    Ok(())
}

async fn inherited_client() -> Result<RunningService<RoleClient, ()>, McpCliError> {
    let socket = var_os(AETHER_MCP_IPC_SOCKET)
        .ok_or_else(|| McpCliError::Runtime("aether mcp is only available inside an active Aether session".into()))?;

    let endpoint = UnixSocketPath::parse(socket)
        .map_err(|error| McpCliError::Runtime(format!("invalid tool gateway environment: {error}")))?;

    let stream = UnixStream::connect(endpoint.socket_path()).await?;
    ().serve(stream).await.map_err(runtime_error)
}

async fn list_tools(client: &RunningService<RoleClient, ()>) -> Result<Vec<ToolDefinition>, McpCliError> {
    let result = timeout(DISCOVERY_TIMEOUT, client.peer().list_all_tools())
        .await
        .map_err(|_| McpCliError::Runtime("timed out waiting for the MCP runtime".into()))?
        .map_err(runtime_error)?;

    Ok(result
        .into_iter()
        .filter(|tool| tool.name != LIST_SERVERS_TOOL)
        .filter_map(|tool| {
            let (server, name) = split_on_server_name(&tool.name)?;
            Some(
                ToolDefinition::new(
                    name,
                    tool.description.unwrap_or_default(),
                    Value::Object((*tool.input_schema).clone()),
                )
                .with_server(server),
            )
        })
        .collect())
}

async fn request(
    client: &RunningService<RoleClient, ()>,
    request: CallToolRequestParams,
    timeout_duration: Duration,
) -> Result<CallToolResult, McpCliError> {
    timeout(timeout_duration, client.peer().call_tool(request))
        .await
        .map_err(|_| McpCliError::Runtime("timed out waiting for the MCP runtime".into()))?
        .map_err(runtime_error)
}

fn tool_result_value(result: CallToolResult) -> Result<Value, McpCliError> {
    if result.is_error == Some(true) {
        return Err(McpCliError::Runtime(format!("MCP tool failed: {:?}", result.content)));
    }
    result
        .structured_content
        .or_else(|| serde_json::to_value(result.content).ok())
        .ok_or_else(|| McpCliError::Runtime("MCP tool returned an invalid result".into()))
}

fn describe_tool(tool: &ToolDefinition) -> String {
    let server = tool.server.as_deref().unwrap_or("<server>");
    format!(
        "{}\n\nUsage: aether mcp {} {} [key=value ... | --key value ... | --args '<JSON object>']\n       printf '%s' '<JSON object>' | aether mcp {} {}\n\nInput schema:\n{}",
        tool.description,
        server,
        tool.name,
        server,
        tool.name,
        serde_json::to_string_pretty(&tool.parameters).unwrap_or_else(|_| "{}".to_string())
    )
}

fn parse_arguments(args: &[&str], stdin: &str) -> Result<Map<String, Value>, McpCliError> {
    match (args, stdin.trim()) {
        ([], "") => Ok(Map::new()),
        ([], stdin) => parse_object(stdin, "stdin"),
        (["--args"], "") => Err(usage("--args requires a JSON object")),
        (["--args", object], "") => parse_object(object, "--args"),
        (args, "") if !args.contains(&"--args") => parse_fields(args),
        _ => Err(usage("--args, field arguments, and stdin are mutually exclusive")),
    }
}

fn parse_fields(args: &[&str]) -> Result<Map<String, Value>, McpCliError> {
    let mut fields = Map::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (key, value) = match arg.strip_prefix("--") {
            Some("") => break,
            Some("help") => return Err(usage(format!("invalid field argument '{arg}'"))),
            Some(key) => (key, *rest.next().ok_or_else(|| usage(format!("--{key} requires a value")))?),
            None => split_pair(arg)?,
        };
        insert_field(&mut fields, key, value)?;
    }
    for arg in rest {
        let (key, value) = split_pair(arg)?;
        insert_field(&mut fields, key, value)?;
    }
    Ok(fields)
}

fn split_pair(arg: &str) -> Result<(&str, &str), McpCliError> {
    match arg.split_once('=') {
        Some(("", _)) => Err(usage("field name cannot be empty")),
        Some(pair) => Ok(pair),
        None => Err(usage(format!("expected key=value or --key value, got '{arg}'"))),
    }
}

fn insert_field(fields: &mut Map<String, Value>, key: &str, value: &str) -> Result<(), McpCliError> {
    let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
    match fields.insert(key.to_string(), value) {
        Some(_) => Err(usage(format!("duplicate field '{key}'"))),
        None => Ok(()),
    }
}

fn parse_object(input: &str, source: &str) -> Result<Map<String, Value>, McpCliError> {
    serde_json::from_str::<Value>(input)
        .map_err(|error| usage(format!("invalid JSON from {source}: {error}")))?
        .as_object()
        .cloned()
        .ok_or_else(|| usage(format!("{source} must contain a JSON object")))
}

fn read_stdin_if_piped() -> Result<String, McpCliError> {
    if std::io::stdin().is_terminal() {
        return Ok(String::new());
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    Ok(input)
}

fn runtime_error(error: impl std::fmt::Display) -> McpCliError {
    McpCliError::Runtime(error.to_string())
}

fn usage(message: impl Into<String>) -> McpCliError {
    McpCliError::Usage(message.into())
}

fn one_line(value: &str) -> String {
    value.lines().next().unwrap_or_default().to_string()
}
