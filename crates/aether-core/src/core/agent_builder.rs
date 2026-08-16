use super::agent::{AgentConfig, AutoContinue, RetryConfig};
use crate::agent_spec::AgentSpec;
use crate::context::CompactionConfig;
use crate::core::{Agent, AgentDeps, Prompt, PromptCache, Result};
use crate::events::{AgentEvent, AgentObserver, Command};
use crate::mcp::McpCommandClient;
use llm::parser::ModelProviderParser;
use llm::types::IsoString;
use llm::{ChatMessage, Context, ModelSettings, StreamingModelProvider, ToolDefinition};
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
    mcp_tx: Option<McpCommandClient>,
    channel_capacity: usize,
    tool_timeout: Duration,
    compaction_config: Option<CompactionConfig>,
    max_auto_continues: u32,
    retry_config: RetryConfig,
    context_window: Option<u32>,
    model_settings: ModelSettings,
    observers: Vec<Box<dyn AgentObserver>>,
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
            context_window: None,
            model_settings: ModelSettings::default(),
            observers: Vec::new(),
        }
    }

    /// Create a builder from a resolved `AgentSpec`.
    ///
    /// The LLM provider is derived from `spec.model` via `ModelProviderParser`.
    /// `base_prompts` are prepended before the spec's own prompts.
    pub async fn from_spec(spec: &AgentSpec, base_prompts: Vec<Prompt>, deps: &AgentDeps) -> Result<Self> {
        let parser = ModelProviderParser::default().with_provider_connections(spec.provider_connections.clone());
        let parser = match deps.oauth_credential_store.clone() {
            Some(store) => parser.with_codex_provider(store),
            None => parser,
        };
        let (provider, _) = parser.parse(&spec.model).await?;
        let mut builder = Self::new(Arc::from(provider))
            .context_window(spec.context_window)
            .model_settings(spec.model_settings.clone());

        if let Some(observer) = deps.observer(&spec.name) {
            builder = builder.observer(observer);
        }

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

    pub fn tools(mut self, tx: McpCommandClient, tools: Vec<ToolDefinition>) -> Self {
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
    /// before giving up and ending the turn with [`TurnEvent::Ended`](crate::events::TurnEvent::Ended).
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

    /// Override the effective model context window in tokens.
    pub fn context_window(mut self, context_window: Option<u32>) -> Self {
        self.context_window = context_window;
        self
    }

    /// Set the sampling controls (`temperature`, `top_p`, `max_tokens`) applied to
    /// every model call this agent makes.
    pub fn model_settings(mut self, model_settings: ModelSettings) -> Self {
        self.model_settings = model_settings;
        self
    }

    /// Pre-populate the context with conversation history (e.g. from a restored session).
    ///
    /// These messages are inserted after the system prompt.
    pub fn messages(mut self, messages: Vec<ChatMessage>) -> Self {
        self.initial_messages = messages;
        self
    }

    /// Attach an observer of the agent's event stream.
    pub fn observer(mut self, observer: Box<dyn AgentObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    pub async fn spawn(self) -> Result<(Sender<Command>, Receiver<AgentEvent>, AgentHandle)> {
        let mut prompt_cache = PromptCache::new(self.prompts);
        let system_content = prompt_cache.render().await?;
        let mut messages = Vec::new();

        if !system_content.is_empty() {
            messages.push(ChatMessage::System { content: system_content, timestamp: IsoString::now() });
        }

        messages.extend(self.initial_messages);
        let (command_tx, command_rx) = mpsc::channel::<Command>(self.channel_capacity);
        let (message_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(self.channel_capacity);
        let mut context = Context::new(messages, self.tool_definitions);
        context.set_model_settings(self.model_settings);

        let config = AgentConfig {
            llm: self.llm,
            context,
            mcp_command_tx: self.mcp_tx,
            tool_timeout: self.tool_timeout,
            compaction_config: self.compaction_config,
            auto_continue: AutoContinue::new(self.max_auto_continues),
            retry_config: self.retry_config,
            context_window: self.context_window,
            prompt_cache,
            observers: self.observers,
        };

        let agent = Agent::new(config, command_rx, message_tx);
        let agent_handle = tokio::spawn(agent.run());

        Ok((command_tx, agent_event_rx, AgentHandle { handle: agent_handle }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_spec::{AgentSpecExposure, ToolFilter};
    use llm::ProviderConnectionOverrides;

    #[tokio::test]
    async fn test_agent_handle_is_finished() {
        let handle = AgentHandle { handle: tokio::spawn(async {}) };
        handle.await_completion().await;
    }

    #[tokio::test]
    async fn test_agent_handle_abort() {
        let handle = AgentHandle { handle: tokio::spawn(std::future::pending::<()>()) };
        assert!(!handle.is_finished());
        handle.abort();
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
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
    async fn from_spec_applies_context_window_and_model_settings() {
        let settings = ModelSettings { temperature: Some(0.0), max_tokens: Some(128), ..Default::default() };
        let spec = AgentSpec {
            name: "alloy".to_string(),
            description: "alloy".to_string(),
            model: "ollama:llama3.2,llamacpp:local".to_string(),
            reasoning_effort: None,
            model_settings: settings.clone(),
            context_window: Some(200_000),
            prompts: vec![],
            provider_connections: ProviderConnectionOverrides::default(),
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::both(),
            tools: ToolFilter::default(),
        };

        let builder = AgentBuilder::from_spec(&spec, vec![], &AgentDeps::default()).await.unwrap();

        assert_eq!(builder.context_window, Some(200_000));
        assert_eq!(builder.model_settings, settings);
    }

    #[tokio::test]
    async fn from_spec_accepts_alloy_model_specs() {
        let spec = AgentSpec {
            name: "alloy".to_string(),
            description: "alloy".to_string(),
            model: "ollama:llama3.2,llamacpp:local".to_string(),
            reasoning_effort: None,
            model_settings: ModelSettings::default(),
            context_window: None,
            prompts: vec![],
            provider_connections: ProviderConnectionOverrides::default(),
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::both(),
            tools: ToolFilter::default(),
        };

        let builder = AgentBuilder::from_spec(&spec, vec![], &AgentDeps::default()).await;
        assert!(builder.is_ok());
    }
}
