use super::{AETHER_MCP_IPC_SOCKET, LIST_SERVERS_TOOL, UnixSocketPath, connect};
use crate::request_context::{AETHER_MCP_REQUEST_CONTEXT, GatewayRequestContext, RequestContextError};
use clap::{ArgAction, Args};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResponse, CallToolResult, MetaObject, PaginatedRequestParams, Tool},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    env::var_os,
    fmt::Display,
    future::Future,
    io::{self, IsTerminal, Read},
    path::PathBuf,
    time::Duration,
};

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
    #[arg(long = "timeout", value_name = "SECONDS", default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..), requires = "tool")]
    timeout_seconds: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum McpCommandError {
    #[error("{0}")]
    Usage(String),
    #[error("MCP commands require the inherited {AETHER_MCP_IPC_SOCKET} from an active Aether runtime")]
    SessionUnavailable,
    #[error("invalid {AETHER_MCP_IPC_SOCKET}: {0}")]
    InvalidSocket(String),
    #[error("invalid {AETHER_MCP_REQUEST_CONTEXT}: {0}")]
    InvalidContext(#[from] RequestContextError),
    #[error("failed to connect to the active Aether runtime: {0}")]
    Connect(String),
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

impl McpCommandError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::Stdin(_) | Self::InvalidContext(_) => 2,
            _ => 1,
        }
    }
}

/// Run the same progressive-discovery command in either installed executable.
pub async fn run(args: McpArgs, command_name: &str) -> Result<(), McpCommandError> {
    let request = Request::parse(args, command_name)?;
    let meta = inherited_metadata()?;
    let path = var_os(AETHER_MCP_IPC_SOCKET).ok_or(McpCommandError::SessionUnavailable)?;
    let socket = UnixSocketPath::from_path(PathBuf::from(path))
        .map_err(|error| McpCommandError::InvalidSocket(error.to_string()))?;
    let transport = connect(socket.path()).await.map_err(|error| McpCommandError::Connect(error.to_string()))?;
    let client = ().serve(transport).await.map_err(|error| McpCommandError::Connect(error.to_string()))?;
    let command = Command { client: &client, name: command_name, meta };
    command.execute(request).await
}

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

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

struct Command<'a> {
    client: &'a rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &'a str,
    meta: Option<MetaObject>,
}

impl Request {
    fn parse(args: McpArgs, command: &str) -> Result<Self, McpCommandError> {
        match (args.server, args.tool, args.help) {
            (None, None, _) => Ok(Self::Help(HelpLevel::Servers)),
            (Some(server), None, true) => Ok(Self::Help(HelpLevel::Server(server))),
            (Some(server), Some(tool), true) => Ok(Self::Help(HelpLevel::Tool { server, tool })),
            (Some(server), Some(tool), false) => {
                let json = if let Some(input) = args.json.as_deref() {
                    parse_json_object(Some(input))?
                } else {
                    parse_json_object(read_stdin()?.as_deref())?
                };
                Ok(Self::Call { server, tool, json, timeout_seconds: args.timeout_seconds })
            }
            _ => Err(McpCommandError::Usage(format!(
                "usage: {command} <server> <tool> [--json <object>] [--timeout <seconds>]"
            ))),
        }
    }
}

impl Command<'_> {
    async fn execute(&self, request: Request) -> Result<(), McpCommandError> {
        match request {
            Request::Help(HelpLevel::Servers) => self.show_servers_help().await,
            Request::Help(HelpLevel::Server(server)) => self.show_server_help(&server).await,
            Request::Help(HelpLevel::Tool { server, tool }) => self.show_tool_help(&server, &tool).await,
            Request::Call { server, tool, json, timeout_seconds } => {
                let params = CallToolRequestParams::new(format!("{server}__{tool}")).with_arguments(json);
                let result = self.call(params, Duration::from_secs(timeout_seconds)).await?;
                if result.is_error.unwrap_or(false) {
                    return Err(McpCommandError::Tool(result_json(&result).to_string()));
                }
                println!("{}", serde_json::to_string(&result_json(&result)).map_err(McpCommandError::Output)?);
                Ok(())
            }
        }
    }

    async fn call(
        &self,
        mut params: CallToolRequestParams,
        timeout: Duration,
    ) -> Result<CallToolResult, McpCommandError> {
        params.meta = self.meta.clone().map(Into::into);
        match timed(timeout, self.client.call_tool_once(params)).await? {
            CallToolResponse::Complete(result) => Ok(result),
            _ => Err(McpCommandError::Request("gateway returned an incomplete response".into())),
        }
    }

    async fn list_tools(&self) -> Result<Vec<Tool>, McpCommandError> {
        timed(DISCOVERY_TIMEOUT, async {
            let mut tools = Vec::new();
            let mut cursor = None;
            loop {
                let mut params = PaginatedRequestParams::default().with_cursor(cursor);
                params.meta = self.meta.clone().map(Into::into);
                let page = self.client.list_tools(Some(params)).await?;
                tools.extend(page.tools);
                cursor = page.next_cursor;
                if cursor.is_none() {
                    return Ok::<_, rmcp::ServiceError>(tools);
                }
            }
        })
        .await
    }

    async fn show_servers_help(&self) -> Result<(), McpCommandError> {
        let result = self.call(CallToolRequestParams::new(LIST_SERVERS_TOOL), DISCOVERY_TIMEOUT).await?;
        let servers: Vec<ServerSummary> = serde_json::from_value(
            result
                .structured_content
                .ok_or_else(|| McpCommandError::Request("server discovery returned no JSON".into()))?,
        )
        .map_err(|error| McpCommandError::Request(format!("invalid server discovery response: {error}")))?;
        println!("Discover and call deferred MCP tools through the active Aether session.\n");
        println!("Usage: {} <server> --help\n", self.name);
        println!("Deferred MCP servers:");
        for server in servers {
            println!("  {:<20} {}", server.name, server.description);
        }
        Ok(())
    }

    async fn show_server_help(&self, server: &str) -> Result<(), McpCommandError> {
        let tools = self.list_tools().await?;
        let prefix = format!("{server}__");
        let tools: Vec<_> = tools.iter().filter(|tool| tool.name.starts_with(&prefix)).collect();
        if tools.is_empty() {
            return Err(McpCommandError::Usage(format!("unknown deferred server `{server}`")));
        }
        println!("Deferred tools from `{server}`.\n");
        println!("Usage: {} {server} <tool> --help\n", self.name);
        println!("Tools:");
        for tool in tools {
            let local_name = tool.name.strip_prefix(&prefix).unwrap_or(tool.name.as_ref());
            println!("  {:<20} {}", local_name, tool.description.as_deref().unwrap_or_default());
        }
        Ok(())
    }

    async fn show_tool_help(&self, server: &str, tool: &str) -> Result<(), McpCommandError> {
        let namespaced = format!("{server}__{tool}");
        let definition = self
            .list_tools()
            .await?
            .into_iter()
            .find(|definition| definition.name == namespaced)
            .ok_or_else(|| McpCommandError::Usage(format!("unknown deferred tool `{server} {tool}`")))?;
        println!("{}\n", definition.description.as_deref().unwrap_or_default());
        println!("Usage:");
        println!("  {} {server} {tool} --json '{{...}}'", self.name);
        println!("  printf '%s' '{{...}}' | {} {server} {tool}\n", self.name);
        println!("Input schema:");
        println!(
            "{}",
            serde_json::to_string_pretty(definition.input_schema.as_ref()).map_err(McpCommandError::Output)?
        );
        Ok(())
    }
}

fn inherited_metadata() -> Result<Option<MetaObject>, McpCommandError> {
    let Some(raw) = var_os(AETHER_MCP_REQUEST_CONTEXT) else { return Ok(None) };
    let raw = raw.to_str().ok_or(RequestContextError::Invalid)?;
    let context = GatewayRequestContext::from_json(raw)?;
    let mut meta = MetaObject::default();
    context.merge_into(&mut meta)?;
    Ok(Some(meta))
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

async fn timed<T, U: Display>(
    duration: Duration,
    future: impl Future<Output = Result<T, U>>,
) -> Result<T, McpCommandError> {
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| McpCommandError::Timeout(duration.as_secs()))?
        .map_err(|error| McpCommandError::Request(error.to_string()))
}

fn result_json(result: &CallToolResult) -> Value {
    result.structured_content.clone().unwrap_or_else(|| serde_json::to_value(&result.content).unwrap_or(Value::Null))
}
