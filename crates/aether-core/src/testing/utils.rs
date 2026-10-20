use mcp_utils::client::ElicitationSink;
use mcp_utils::gateway::ServerSpec;
use mcp_utils::model::ElicitResult;
use mcp_utils::server::McpServer;
use mcp_utils::testing::{CapturedElicitation, ElicitationScriptBuilder, FakeMcpServer};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

use crate::context::CompactionConfig;
use crate::core::{AgentBuilder, AgentError, McpInstructions, Prompt, RetryConfig, agent};
use crate::events::{
    AgentCommand, AgentEvent, AgentObserver, Command, ContextEvent, ToolEvent, TurnEvent, UserCommand,
};
use crate::mcp::{McpBuilder, McpCatalog, mcp};
use crate::testing::{AgentTrace, FakeAgentObserver};
use llm::{ChatMessage, Context, LlmError, LlmModel, LlmResponse, ModelSettings, StreamingModelProvider};

use llm::testing::FakeLlmProvider;

pub async fn drain_until(
    receiver: &mut mpsc::Receiver<AgentEvent>,
    predicate: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        let matched = predicate(&event);
        events.push(event);
        if matched {
            return events;
        }
    }
    panic!("agent event channel closed before predicate matched");
}

pub fn content_events(events: Vec<AgentEvent>) -> Vec<AgentEvent> {
    events
        .into_iter()
        .filter(|event| {
            !matches!(
                event,
                AgentEvent::Turn(
                    TurnEvent::Started { .. }
                        | TurnEvent::UserMessageInserted { .. }
                        | TurnEvent::UserMessageDiscarded { .. }
                        | TurnEvent::LlmCallStarted { .. }
                        | TurnEvent::LlmCallEnded { .. }
                ) | AgentEvent::Tool(ToolEvent::DefinitionsUpdated { .. })
                    | AgentEvent::Context(ContextEvent::ReplyRecorded { .. })
            )
        })
        .collect()
}

pub async fn ready_catalog(builder: McpBuilder) -> McpCatalog {
    let catalog = builder.spawn().expect("the MCP catalog spawns");
    catalog.ready().await;
    catalog
}

pub fn mcp_instructions(entries: &[(&str, &str)]) -> McpInstructions {
    let servers = entries.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    McpInstructions { servers, progressive_discovery: false }
}

/// Millisecond-scale retry delays so retry tests run fast under virtual time.
pub fn fast_retry(max_attempts: u32) -> RetryConfig {
    RetryConfig { max_attempts, base_delay: Duration::from_millis(1), max_delay: Duration::from_millis(5) }
}

pub fn test_agent() -> TestAgentBuilder {
    TestAgentBuilder::default()
}

/// A fluent sequence of commands and synchronization points for a test agent.
#[derive(Default)]
pub struct TestScenario {
    steps: Vec<Step>,
}

impl TestScenario {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn send(self, command: Command) -> Self {
        self.step(Step::Send(command))
    }

    pub fn user_text(self, text: impl Into<String>) -> Self {
        self.send(Command::text(text))
    }

    pub fn cancel(self) -> Self {
        self.send(Command::UserCommand(UserCommand::Cancel))
    }

    pub fn switch_model(self, provider: impl StreamingModelProvider + 'static) -> Self {
        self.send(Command::AgentCommand(AgentCommand::SwitchModel(Box::new(provider))))
    }

    pub fn replace_conversation(self, messages: Vec<ChatMessage>) -> Self {
        self.send(Command::AgentCommand(AgentCommand::ReplaceConversation(messages)))
    }

    pub fn wait_for(self, predicate: impl Fn(&AgentEvent) -> bool + Send + 'static) -> Self {
        self.step(Step::WaitFor(Box::new(predicate)))
    }

    pub fn wait_for_turn_end(self) -> Self {
        self.wait_for(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. })))
    }

    pub fn wait_for_compaction_start(self) -> Self {
        self.wait_for(|event| matches!(event, AgentEvent::Context(ContextEvent::CompactionStarted { .. })))
    }

    pub fn wait_for_retry(self, attempt: u32) -> Self {
        self.wait_for(
            move |event| matches!(event, AgentEvent::Turn(TurnEvent::RetryScheduled { attempt: actual, .. }) if *actual == attempt),
        )
    }

    /// Run an arbitrary side effect between scenario steps, e.g. releasing a
    /// paused LLM stream so a queued message can be injected mid-turn.
    pub fn perform(self, action: impl FnOnce() + Send + 'static) -> Self {
        self.step(Step::Perform(Box::new(action)))
    }

    fn step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }
}

/// Result of running a test agent, including messages, captured contexts, and the MCP elicitations it answered.
pub struct TestAgentResult {
    pub messages: Vec<AgentEvent>,
    pub contexts: Vec<Context>,
    pub elicitations: Vec<CapturedElicitation>,
}

/// Error returned by [`TestAgentBuilder::run`], [`TestAgentBuilder::run_trace`], and
/// [`TestAgentBuilder::run_with_context`].
#[derive(Debug, thiserror::Error)]
pub enum TestAgentError {
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error("failed to send command to the test agent: {0}")]
    SendCommand(#[from] mpsc::error::SendError<Command>),
}

pub type TestResult<T> = std::result::Result<T, TestAgentError>;

#[derive(Default)]
pub struct TestAgentBuilder {
    responses: Vec<Vec<Result<LlmResponse, LlmError>>>,
    llm_config: Vec<Configure<FakeLlmProvider>>,
    agent_config: Vec<Configure<AgentBuilder>>,
    /// `None` connects a single fake server named `test`.
    mcp_servers: Option<Vec<ServerSpec>>,
    elicitations: Option<ElicitationScriptBuilder>,
    scenario: Option<TestScenario>,
}

impl TestAgentBuilder {
    pub fn commands(self, commands: Vec<Command>) -> Self {
        assert!(!commands.is_empty(), "commands() requires at least one command");
        let scenario = commands.into_iter().fold(TestScenario::new(), TestScenario::send);
        self.scenario(scenario.wait_for_turn_end())
    }

    pub fn scenario(mut self, scenario: TestScenario) -> Self {
        assert!(!scenario.steps.is_empty(), "scenario() requires at least one step");
        assert!(self.scenario.is_none(), "commands(), user_text(), and scenario() are mutually exclusive");
        self.scenario = Some(scenario);
        self
    }

    pub fn user_text(self, text: &str) -> Self {
        self.commands(vec![Command::text(text)])
    }

    pub fn llm_responses(mut self, llm_responses: &[Vec<LlmResponse>]) -> Self {
        self.responses = llm_responses.iter().map(|turn| turn.iter().cloned().map(Ok).collect()).collect();
        self
    }

    pub fn llm_result_responses(mut self, llm_responses: &[Vec<Result<LlmResponse, LlmError>>]) -> Self {
        self.responses = Vec::from(llm_responses);
        self
    }

    pub fn model(self, model: LlmModel) -> Self {
        self.configure_llm(|llm| llm.with_model(model))
    }

    pub fn provider_context_window(self, window: Option<u32>) -> Self {
        self.configure_llm(move |llm| llm.with_context_window(window))
    }

    pub fn context_window_override(self, window: u32) -> Self {
        self.configure(move |agent| agent.context_window(Some(window)))
    }

    pub fn tool_timeout(self, timeout: Duration) -> Self {
        self.configure(move |agent| agent.tool_timeout(timeout))
    }

    pub fn max_auto_continues(self, max: u32) -> Self {
        self.configure(move |agent| agent.max_auto_continues(max))
    }

    pub fn retry_config(self, config: RetryConfig) -> Self {
        self.configure(move |agent| agent.retry(config))
    }

    /// Run without the default fake MCP server when the scenario does not exercise tools.
    pub fn without_mcp(mut self) -> Self {
        self.mcp_servers = Some(Vec::new());
        self
    }

    pub fn mcp_server(mut self, name: &str, server: impl Into<McpServer>) -> Self {
        self.mcp_servers.get_or_insert_with(Vec::new).push(ServerSpec::in_process(name, server));
        self
    }

    pub fn elicitation_response(mut self, response: ElicitResult) -> Self {
        self.elicitations = Some(self.elicitations.take().unwrap_or_default().response(response));
        self
    }

    pub fn on_url_elicitation<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(String, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.elicitations = Some(self.elicitations.take().unwrap_or_default().on_url(handler));
        self
    }

    /// Pre-populate the context with conversation history.
    pub fn messages(self, messages: Vec<ChatMessage>) -> Self {
        self.configure(|agent| agent.messages(messages))
    }

    /// Set the system prompt. Multiple prompts are concatenated with double newlines.
    pub fn system_prompt(self, prompt: Prompt) -> Self {
        self.configure(|agent| agent.system_prompt(prompt))
    }

    /// Set the session affinity key attached to every LLM call; defaults to a
    /// fresh random key per agent.
    pub fn session_affinity_key(self, key: impl Into<String>) -> Self {
        let key = key.into();
        self.configure(|agent| agent.session_affinity_key(key))
    }

    /// Configure context compaction settings.
    pub fn compaction_config(self, config: CompactionConfig) -> Self {
        self.configure(|agent| agent.compaction(config))
    }

    /// Set the model settings applied to every LLM call.
    pub fn model_settings(self, settings: ModelSettings) -> Self {
        self.configure(|agent| agent.model_settings(settings))
    }

    /// Pause the fake LLM stream at `turn_index` / `chunk_index` until
    /// `release.notify_one()` is called. Used for deterministic timing tests.
    pub fn pause_turn_after(self, turn_index: usize, chunk_index: usize, release: Arc<Notify>) -> Self {
        self.configure_llm(move |llm| llm.pause_turn_after(turn_index, chunk_index, release))
    }

    /// Attach an observer of the test agent's event stream.
    pub fn observer(self, observer: Box<dyn AgentObserver>) -> Self {
        self.configure(|agent| agent.observer(observer))
    }

    pub async fn run(self) -> TestResult<Vec<AgentEvent>> {
        let result = self.run_with_context().await?;
        Ok(result.messages)
    }

    /// Runs the test agent with a recording observer attached and returns the
    /// full event trace, including internal events.
    pub async fn run_trace(self) -> TestResult<AgentTrace> {
        let observer = FakeAgentObserver::new();
        let events = observer.events();
        self.observer(Box::new(observer)).run().await?;
        Ok(AgentTrace::from_observer_events(&events))
    }

    /// Runs the test agent and returns both messages and captured contexts.
    ///
    /// Use this when you need to verify what context was passed to the LLM,
    /// for example when testing that file attachments are properly formatted.
    pub async fn run_with_context(self) -> TestResult<TestAgentResult> {
        let Self { responses, llm_config, agent_config, mcp_servers, elicitations, scenario } = self;
        let scenario = scenario.expect("test agent requires commands(), user_text(), or scenario()");
        let llm =
            llm_config.into_iter().fold(FakeLlmProvider::from_results(responses), |llm, configure| configure(llm));
        let captured_contexts = llm.captured_contexts();

        let servers = mcp_servers.unwrap_or_else(|| vec![ServerSpec::in_process("test", FakeMcpServer::math())]);
        let mut mcp = mcp("/workspace").with_servers(servers);
        let mut script = None;
        if let Some(elicitations) = elicitations {
            let (sink, events) = mpsc::channel(32);
            mcp = mcp.with_elicitation(ElicitationSink::form_and_url(sink));
            script = Some(elicitations.spawn(events));
        }

        let builder = agent(llm).mcp(ready_catalog(mcp).await).retry(RetryConfig::disabled());
        let builder = agent_config.into_iter().fold(builder, |builder, configure| configure(builder));
        let (tx, mut rx, handle) = builder.spawn().await?;
        let mut messages = Vec::new();

        for step in scenario.steps {
            match step {
                Step::Send(command) => tx.send(command).await?,
                Step::WaitFor(predicate) => loop {
                    let message = rx.recv().await.expect("agent event channel closed before scenario step matched");
                    let matched = predicate(&message);
                    messages.push(message);
                    if matched {
                        break;
                    }
                },
                Step::Perform(action) => action(),
            }
        }
        drop(tx);

        handle.await_completion().await;

        let contexts = captured_contexts.lock().expect("captured contexts lock poisoned").clone();
        let elicitations = script.map(|script| script.captured()).unwrap_or_default();
        Ok(TestAgentResult { messages, contexts, elicitations })
    }

    fn configure(mut self, configure: impl FnOnce(AgentBuilder) -> AgentBuilder + Send + 'static) -> Self {
        self.agent_config.push(Box::new(configure));
        self
    }

    fn configure_llm(mut self, configure: impl FnOnce(FakeLlmProvider) -> FakeLlmProvider + Send + 'static) -> Self {
        self.llm_config.push(Box::new(configure));
        self
    }
}

type Configure<T> = Box<dyn FnOnce(T) -> T + Send>;

enum Step {
    Send(Command),
    WaitFor(Box<dyn Fn(&AgentEvent) -> bool + Send>),
    Perform(Box<dyn FnOnce() + Send>),
}
