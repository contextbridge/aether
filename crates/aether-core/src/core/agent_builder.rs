use super::agent::RetryConfig;
use crate::agent_spec::AgentSpec;
use crate::context::{CompactionConfig, SessionUsageTracker};
use crate::core::{Agent, AgentDeps, Prompt, PromptCache, Result};
use crate::events::{AgentEvent, AgentObserver, Command, TurnOutcome};
use crate::mcp::McpCatalog;
use llm::parser::ModelProviderParser;
use llm::{ChatMessage, Context, ModelSettings, SessionUsageEvent, StreamingModelProvider};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_util::task::AbortOnDropHandle;

/// Handle to a running agent's task; dropping it aborts the agent.
pub struct AgentHandle {
    handle: AbortOnDropHandle<()>,
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

pub async fn recv_agent_event(rx: &mut Receiver<AgentEvent>) -> AgentEvent {
    rx.recv()
        .await
        .unwrap_or_else(|| AgentEvent::turn_ended(TurnOutcome::failed("Agent stopped before its turn ended")))
}

pub struct AgentBuilder {
    pub(super) llm: Arc<dyn StreamingModelProvider>,
    pub(super) prompts: Vec<Prompt>,
    pub(super) initial_messages: Vec<ChatMessage>,
    pub(super) mcp: McpCatalog,
    pub(super) channel_capacity: usize,
    pub(super) tool_timeout: Duration,
    pub(super) compaction_config: Option<CompactionConfig>,
    pub(super) max_auto_continues: u32,
    pub(super) retry_config: RetryConfig,
    pub(super) context_window: Option<u32>,
    pub(super) model_settings: ModelSettings,
    pub(super) observers: Vec<Box<dyn AgentObserver>>,
    pub(super) session_usage: SessionUsageTracker,
    pub(super) session_affinity_key: String,
}

impl AgentBuilder {
    pub fn new(llm: Arc<dyn StreamingModelProvider>) -> Self {
        Self {
            llm,
            prompts: Vec::new(),
            initial_messages: Vec::new(),
            mcp: McpCatalog::default(),
            channel_capacity: 1000,
            tool_timeout: Duration::from_mins(60),
            compaction_config: Some(CompactionConfig::default()),
            max_auto_continues: 3,
            retry_config: RetryConfig::default(),
            context_window: None,
            model_settings: ModelSettings::default(),
            observers: Vec::new(),
            session_usage: SessionUsageTracker::new("agent"),
            session_affinity_key: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Create a builder from a resolved `AgentSpec`.
    ///
    /// The LLM provider is derived from `spec.model` via `ModelProviderParser`.
    pub async fn from_spec(spec: &AgentSpec, deps: &AgentDeps) -> Result<Self> {
        let parser = ModelProviderParser::default().with_provider_connections(spec.provider_connections.clone());
        let parser = match deps.oauth_credential_store.clone() {
            Some(store) => parser.with_codex_provider(store),
            None => parser,
        };
        let (provider, _) = parser.parse(&spec.model).await?;
        Ok(Self::from_spec_with_llm(spec, deps, Arc::from(provider)))
    }

    pub fn from_spec_with_llm(spec: &AgentSpec, deps: &AgentDeps, llm: Arc<dyn StreamingModelProvider>) -> Self {
        let mut builder = Self::new(llm)
            .context_window(spec.context_window)
            .model_settings(spec.model_settings.clone())
            .session_usage(SessionUsageTracker::new(&spec.name));

        if let Some(key) = &deps.session_affinity_key {
            builder = builder.session_affinity_key(key.clone());
        }
        if let Some(observer) = deps.observer(&spec.name) {
            builder = builder.observer(observer);
        }

        for prompt in &spec.prompts {
            builder = builder.system_prompt(prompt.clone());
        }

        builder
    }

    /// Add a prompt to the system prompt.
    ///
    /// Multiple prompts are concatenated with double newlines.
    pub fn system_prompt(mut self, prompt: Prompt) -> Self {
        self.prompts.push(prompt);
        self
    }

    pub fn mcp(mut self, mcp: McpCatalog) -> Self {
        self.mcp = mcp;
        self
    }

    /// Set the timeout for tool execution
    ///
    /// If a tool does not return a result within this duration, it will be marked as failed
    /// and the agent will continue processing.
    ///
    /// Default: 60 minutes
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

    pub fn session_affinity_key(mut self, key: impl Into<String>) -> Self {
        self.session_affinity_key = key.into();
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

    /// Record usage under `tracker`, which names this agent in usage events.
    pub fn session_usage(mut self, tracker: SessionUsageTracker) -> Self {
        self.session_usage = tracker;
        self
    }

    /// Continue session totals from the last persisted usage event, e.g. when
    /// resuming a session.
    pub fn resume_usage(mut self, last: &SessionUsageEvent) -> Self {
        self.session_usage.resume_from(last);
        self
    }

    pub async fn spawn(mut self) -> Result<(Sender<Command>, Receiver<AgentEvent>, AgentHandle)> {
        let mut prompt_cache = PromptCache::new(std::mem::take(&mut self.prompts));
        let system_content = prompt_cache.render().await?;
        let mut messages = Vec::new();

        if !system_content.is_empty() {
            messages.push(ChatMessage::system(system_content));
        }

        messages.extend(std::mem::take(&mut self.initial_messages));
        let (command_tx, command_rx) = mpsc::channel::<Command>(self.channel_capacity);
        let (message_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(self.channel_capacity);
        let mut context = Context::new(messages, Vec::new());
        context.set_model_settings(std::mem::take(&mut self.model_settings));
        context.set_session_affinity_key(Some(std::mem::take(&mut self.session_affinity_key)));

        let agent = Agent::new(self, context, prompt_cache, command_rx, message_tx);
        let handle = AbortOnDropHandle::new(tokio::spawn(agent.run()));

        Ok((command_tx, agent_event_rx, AgentHandle { handle }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TurnEvent;
    use llm::testing::FakeLlmProvider;

    #[tokio::test]
    async fn closing_the_input_lets_the_agent_finish() {
        let (tx, _rx, handle) = AgentBuilder::new(Arc::new(FakeLlmProvider::new(Vec::new()))).spawn().await.unwrap();

        drop(tx);

        handle.await_completion().await;
    }

    #[tokio::test]
    async fn dropping_the_handle_stops_the_agent() {
        let (_tx, mut rx, handle) =
            AgentBuilder::new(Arc::new(FakeLlmProvider::new(Vec::new()))).spawn().await.unwrap();

        drop(handle);

        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn recv_agent_event_returns_events_from_open_channel() {
        let (tx, mut rx) = mpsc::channel(4);
        tx.send(AgentEvent::Turn(TurnEvent::Started { content: vec![] })).await.unwrap();

        assert_eq!(recv_agent_event(&mut rx).await, AgentEvent::Turn(TurnEvent::Started { content: vec![] }));
    }

    #[tokio::test]
    async fn recv_agent_event_returns_failed_turn_end_once_channel_closes() {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(4);
        drop(tx);

        let event = recv_agent_event(&mut rx).await;

        assert!(matches!(event.turn_outcome(), Some(TurnOutcome::Failed { .. })), "got: {event:?}");
    }

    #[tokio::test]
    async fn from_spec_applies_context_window_and_model_settings() {
        let settings = ModelSettings { temperature: Some(0.0), max_tokens: Some(128), ..Default::default() };
        let spec = AgentSpec {
            model: "ollama:llama3.2,llamacpp:local".to_string(),
            model_settings: settings.clone(),
            context_window: Some(200_000),
            ..AgentSpec::default()
        };

        let dependencies = AgentDeps::default().with_session_affinity_key("conversation-123");
        let builder = AgentBuilder::from_spec(&spec, &dependencies).await.unwrap();

        assert_eq!(builder.context_window, Some(200_000));
        assert_eq!(builder.model_settings, settings);
        assert_eq!(builder.session_affinity_key, "conversation-123");
    }

    #[tokio::test]
    async fn from_spec_accepts_alloy_model_specs() {
        let spec = AgentSpec { model: "ollama:llama3.2,llamacpp:local".to_string(), ..AgentSpec::default() };

        let builder = AgentBuilder::from_spec(&spec, &AgentDeps::default()).await;
        assert!(builder.is_ok());
    }
}
