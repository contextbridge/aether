use mcp_utils::tool_gateway::command;
pub use mcp_utils::tool_gateway::command::{McpArgs, McpCommandError};

pub async fn run(args: McpArgs) -> Result<(), McpCommandError> {
    command::run(args, "aether mcp").await
}
