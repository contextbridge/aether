use crate::agent::Result;
use crate::agent::middleware::{AgentEvent, Middleware, MiddlewareAction};
use crate::agent::{Agent, AgentMessage, UserMessage};
use crate::context::CompactionConfig;
use crate::llm::{ChatMessage, Context, StreamingModelProvider, ToolDefinition};
use crate::mcp::run_mcp_task::McpCommand;
use crate::types::IsoString;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;

/// Handle for communicating with a running Agent
pub struct AgentHandle {
    _agent_handle: JoinHandle<()>,
}

pub struct AgentBuilder<T: StreamingModelProvider> {
    llm: T,
    system_prompt: Option<String>,
    middleware: Middleware,
    tool_definitions: Vec<ToolDefinition>,
    mcp_tx: Option<Sender<McpCommand>>,
    channel_capacity: usize,
    tool_timeout: Duration,
    compaction_config: Option<CompactionConfig>,
}

impl<T: StreamingModelProvider + 'static> AgentBuilder<T> {
    pub fn new(llm: T) -> Self {
        Self {
            llm,
            system_prompt: None,
            middleware: Middleware::new(),
            tool_definitions: Vec::new(),
            mcp_tx: None,
            channel_capacity: 1000,
            tool_timeout: Duration::from_secs(60 * 10),
            compaction_config: Some(CompactionConfig::default()),
        }
    }

    pub fn system(mut self, prompt: &str) -> Self {
        self.system_prompt = Some(prompt.to_string());
        self
    }

    /// Add an event handler for agent events
    ///
    /// Handlers receive all agent events and can pattern match to handle specific ones.
    /// Multiple handlers can be registered and will execute in parallel.
    /// If any handler returns Block, the action will be blocked.
    ///
    /// # Example
    /// ```ignore
    /// agent(llm)
    ///     .on_event(|event| async move {
    ///         match event {
    ///             AgentEvent::ToolCall { name, .. } if name == "dangerous_tool" => {
    ///                 MiddlewareAction::Block
    ///             }
    ///             AgentEvent::AgentDone => {
    ///                 println!("Done!");
    ///                 MiddlewareAction::Allow
    ///             }
    ///             _ => MiddlewareAction::Allow
    ///         }
    ///     })
    /// ```
    pub fn on_event<U, V>(mut self, handler: U) -> Self
    where
        U: Fn(AgentEvent) -> V + Send + Sync + 'static,
        V: Future<Output = MiddlewareAction> + Send + 'static,
    {
        self.middleware.add_handler(handler);
        self
    }

    pub fn tools(mut self, tx: Sender<McpCommand>, tools: Vec<ToolDefinition>) -> Self {
        self.tool_definitions = tools;
        self.mcp_tx = Some(tx);
        self
    }

    /// Set the timeout for tool execution
    ///
    /// If a tool does not return a result within this duration, it will be marked as failed
    /// and the agent will continue processing.
    ///
    /// Default: 10 minutes
    pub fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    /// Configure context compaction settings.
    ///
    /// By default, agents automatically compact context when token usage exceeds
    /// 85% of the context window, preventing overflow during long-running tasks.
    ///
    /// # Examples
    /// ```ignore
    /// // Custom threshold
    /// agent(llm).compaction(CompactionConfig::with_threshold(0.9))
    ///
    /// // Disable compaction entirely
    /// agent(llm).compaction(CompactionConfig::disabled())
    ///
    /// // Full customization
    /// agent(llm).compaction(
    ///     CompactionConfig::with_threshold(0.85)
    ///         .keep_recent_tool_results(3)
    ///         .min_messages(20)
    /// )
    /// ```
    pub fn compaction(mut self, config: CompactionConfig) -> Self {
        self.compaction_config = Some(config);
        self
    }

    pub async fn spawn(self) -> Result<(Sender<UserMessage>, Receiver<AgentMessage>, AgentHandle)> {
        let mut messages = Vec::new();
        if let Some(content) = self.system_prompt {
            messages.push(ChatMessage::System {
                content,
                timestamp: IsoString::now(),
            });
        }

        let (user_message_tx, user_message_rx) =
            mpsc::channel::<UserMessage>(self.channel_capacity);

        let (agent_message_tx, agent_message_rx) =
            mpsc::channel::<AgentMessage>(self.channel_capacity);

        let context = Context::new(messages, self.tool_definitions);

        let agent = Agent::new(
            Arc::new(self.llm),
            context,
            self.mcp_tx,
            user_message_rx,
            agent_message_tx,
            self.middleware,
            self.tool_timeout,
            self.compaction_config,
        );

        let agent_handle = tokio::spawn(agent.run());

        Ok((
            user_message_tx,
            agent_message_rx,
            AgentHandle {
                _agent_handle: agent_handle,
            },
        ))
    }
}
