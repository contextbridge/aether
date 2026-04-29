use super::agent::{AgentConfig, AutoContinue, RetryConfig};
use crate::agent_spec::AgentSpec;
use crate::context::CompactionConfig;
use crate::core::{Agent, Prompt, Result};
use crate::events::{AgentMessage, UserMessage};
use crate::mcp::run_mcp_task::McpCommand;
use llm::parser::ModelProviderParser;
use llm::types::IsoString;
use llm::{ChatMessage, Context, StreamingModelProvider, ToolDefinition};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;

/// Handle for communicating with a running Agent
pub struct AgentHandle {
    handle: JoinHandle<()>,
}

impl AgentHandle {
    /// Abort the agent task immediately.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Returns `true` if the agent task has finished.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Wait for the agent task to complete.
    pub async fn await_completion(self) {
        let _ = self.handle.await;
    }
}

pub struct AgentBuilder {
    llm: Arc<dyn StreamingModelProvider>,
    prompts: Vec<Prompt>,
    tool_definitions: Vec<ToolDefinition>,
    initial_messages: Vec<ChatMessage>,
    mcp_tx: Option<Sender<McpCommand>>,
    channel_capacity: usize,
    tool_timeout: Duration,
    compaction_config: Option<CompactionConfig>,
    max_auto_continues: u32,
    retry_config: RetryConfig,
    prompt_cache_key: Option<String>,
}

impl AgentBuilder {
    pub fn new(llm: Arc<dyn StreamingModelProvider>) -> Self {
        Self {
            llm,
            prompts: Vec::new(),
            tool_definitions: Vec::new(),
            initial_messages: Vec::new(),
            mcp_tx: None,
            channel_capacity: 1000,
            tool_timeout: Duration::from_mins(20),
            compaction_config: Some(CompactionConfig::default()),
            max_auto_continues: 3,
            retry_config: RetryConfig::default(),
            prompt_cache_key: None,
        }
    }

    /// Create a builder from a resolved `AgentSpec`.
    ///
    /// The LLM provider is derived from `spec.model` via `ModelProviderParser`.
    /// `base_prompts` are prepended before the spec's own prompts.
    pub async fn from_spec(spec: &AgentSpec, base_prompts: Vec<Prompt>) -> Result<Self> {
        let (provider, _) = ModelProviderParser::default().parse(&spec.model).await?;
        let mut builder = Self::new(Arc::from(provider));
        for prompt in base_prompts {
            builder = builder.system_prompt(prompt);
        }
        for prompt in &spec.prompts {
            builder = builder.system_prompt(prompt.clone());
        }
        Ok(builder)
    }

    /// Add a prompt to the system prompt.
    ///
    /// Multiple prompts are concatenated with double newlines.
    pub fn system_prompt(mut self, prompt: Prompt) -> Self {
        self.prompts.push(prompt);
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
    /// Default: 20 minutes
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

    /// Disable context compaction entirely.
    ///
    /// Overflow errors from the model will be surfaced directly to callers.
    pub fn disable_compaction(mut self) -> Self {
        self.compaction_config = None;
        self
    }

    /// Configure the maximum number of auto-continue attempts.
    ///
    /// When the LLM stops without making tool calls, the agent may inject a
    /// continuation prompt and restart the LLM stream for resumable stop
    /// reasons (for example, token length limits).
    ///
    /// This setting limits how many times the agent will attempt to continue
    /// before giving up and returning `AgentMessage::Done`.
    ///
    /// Default: 3
    ///
    /// # Example
    /// ```ignore
    /// // Allow up to 5 auto-continue attempts
    /// agent(llm).max_auto_continues(5)
    ///
    /// // Disable auto-continue entirely
    /// agent(llm).max_auto_continues(0)
    /// ```
    pub fn max_auto_continues(mut self, max: u32) -> Self {
        self.max_auto_continues = max;
        self
    }

    /// Configure retry behavior for transient LLM provider failures.
    pub fn retry(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Set a prompt cache key for LLM provider request routing.
    ///
    /// This is typically a session ID (UUID) that remains stable across all
    /// turns within a conversation, improving prompt cache hit rates.
    pub fn prompt_cache_key(mut self, key: String) -> Self {
        self.prompt_cache_key = Some(key);
        self
    }

    /// Pre-populate the context with conversation history (e.g. from a restored session).
    ///
    /// These messages are inserted after the system prompt.
    pub fn messages(mut self, messages: Vec<ChatMessage>) -> Self {
        self.initial_messages = messages;
        self
    }

    pub async fn spawn(self) -> Result<(Sender<UserMessage>, Receiver<AgentMessage>, AgentHandle)> {
        let mut messages = Vec::new();

        if !self.prompts.is_empty() {
            let system_content = Prompt::build_all(&self.prompts).await?;
            if !system_content.is_empty() {
                messages.push(ChatMessage::System { content: system_content, timestamp: IsoString::now() });
            }
        }

        messages.extend(self.initial_messages);

        let (user_message_tx, user_message_rx) = mpsc::channel::<UserMessage>(self.channel_capacity);

        let (message_tx, agent_message_rx) = mpsc::channel::<AgentMessage>(self.channel_capacity);

        let mut context = Context::new(messages, self.tool_definitions);
        context.set_prompt_cache_key(self.prompt_cache_key);

        let config = AgentConfig {
            llm: self.llm,
            context,
            mcp_command_tx: self.mcp_tx,
            tool_timeout: self.tool_timeout,
            compaction_config: self.compaction_config,
            auto_continue: AutoContinue::new(self.max_auto_continues),
            retry_config: self.retry_config,
        };

        let agent = Agent::new(config, user_message_rx, message_tx);

        let agent_handle = tokio::spawn(agent.run());

        Ok((user_message_tx, agent_message_rx, AgentHandle { handle: agent_handle }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_spec::{AgentSpecExposure, ToolFilter};

    #[tokio::test]
    async fn test_agent_handle_is_finished() {
        let handle = AgentHandle { handle: tokio::spawn(async {}) };
        handle.await_completion().await;
    }

    #[tokio::test]
    async fn test_agent_handle_abort() {
        let handle = AgentHandle {
            handle: tokio::spawn(async {
                tokio::time::sleep(Duration::from_mins(1)).await;
            }),
        };
        assert!(!handle.is_finished());
        handle.abort();
        // Give the runtime a moment to process the abort
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(handle.is_finished());
    }

    #[tokio::test]
    async fn system_prompt_preserves_add_order() {
        let builder = AgentBuilder::new(Arc::new(llm::testing::FakeLlmProvider::new(vec![])))
            .system_prompt(Prompt::text("first"))
            .system_prompt(Prompt::text("second"))
            .system_prompt(Prompt::text("third"));

        let rendered = Prompt::build_all(&builder.prompts).await.unwrap();

        assert_eq!(rendered, "first\n\nsecond\n\nthird");
    }

    #[tokio::test]
    async fn from_spec_accepts_alloy_model_specs() {
        let spec = AgentSpec {
            name: "alloy".to_string(),
            description: "alloy".to_string(),
            model: "ollama:llama3.2,llamacpp:local".to_string(),
            reasoning_effort: None,
            prompts: vec![],
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::both(),
            tools: ToolFilter::default(),
        };

        let builder = AgentBuilder::from_spec(&spec, vec![]).await;
        assert!(builder.is_ok());
    }
}
