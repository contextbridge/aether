use llm::{ChatMessage, ContentBlock, ReasoningEffort, StreamingModelProvider, ToolDefinition};

/// The unified command type sent to the agent input channel.
///
/// Wraps both user-facing actions ([`UserCommand`]) and runtime-internal
/// operations ([`AgentCommand`]).
#[derive(Debug)]
pub enum Command {
    UserCommand(UserCommand),
    AgentCommand(AgentCommand),
}

impl Command {
    pub fn text(text: impl Into<String>) -> Self {
        Self::with_content(vec![ContentBlock::text(text)])
    }

    pub fn with_content(content: Vec<ContentBlock>) -> Self {
        Self::UserCommand(UserCommand::Text { content })
    }

    pub fn cancel() -> Self {
        Self::UserCommand(UserCommand::Cancel)
    }

    pub fn clear_context() -> Self {
        Self::UserCommand(UserCommand::ClearContext)
    }

    pub fn agent(command: AgentCommand) -> Self {
        Self::AgentCommand(command)
    }
}

/// User-initiated actions that the agent processes as part of normal interaction.
pub enum UserCommand {
    Text { content: Vec<ContentBlock> },
    Cancel,
    ClearContext,
}

impl std::fmt::Debug for UserCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserCommand::Text { content } => f.debug_struct("Text").field("content_blocks", &content.len()).finish(),
            UserCommand::Cancel => write!(f, "Cancel"),
            UserCommand::ClearContext => write!(f, "ClearContext"),
        }
    }
}

/// Runtime-internal operations that modify agent state without user interaction.
///
/// These are sent by the runtime controller or MCP event pumps to update
/// tools, model, instructions, or to sync conversation history across
/// agent runtimes.
pub enum AgentCommand {
    SwitchModel(Box<dyn StreamingModelProvider>),
    UpdateTools(Vec<ToolDefinition>),
    UpdateMcpInstructions { server: String, body: Option<String> },
    SetReasoningEffort(Option<ReasoningEffort>),
    ReplaceConversation(Vec<ChatMessage>),
}

impl std::fmt::Debug for AgentCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentCommand::SwitchModel(provider) => {
                f.debug_tuple("SwitchModel").field(&provider.display_name()).finish()
            }
            AgentCommand::UpdateTools(tools) => f.debug_tuple("UpdateTools").field(&tools.len()).finish(),
            AgentCommand::UpdateMcpInstructions { server, body } => f
                .debug_struct("UpdateMcpInstructions")
                .field("server", server)
                .field("body_len", &body.as_ref().map(String::len))
                .finish(),
            AgentCommand::SetReasoningEffort(effort) => f.debug_tuple("SetReasoningEffort").field(effort).finish(),
            AgentCommand::ReplaceConversation(messages) => {
                f.debug_tuple("ReplaceConversation").field(&messages.len()).finish()
            }
        }
    }
}
