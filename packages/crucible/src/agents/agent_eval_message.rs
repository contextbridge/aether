/// Accumulated message types for eval logging and judging
#[derive(Debug, Clone)]
pub enum AgentEvalMessage {
    AgentText(String),
    ToolCall { name: String, arguments: String },
    ToolResult { name: String, result: String },
    ToolError(String),
    Error(String),
    Done,
}
