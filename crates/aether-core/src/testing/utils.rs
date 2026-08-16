use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

use crate::context::CompactionConfig;
use crate::core::{AgentError, Prompt, RetryConfig, agent};
use crate::events::{
    AgentCommand, AgentEvent, AgentObserver, Command, ContextEvent, ToolEvent, TurnEvent, UserCommand,
};
use crate::mcp::mcp;
use crate::testing::fake_mcp::fake_mcp;
use crate::testing::{AgentTrace, FakeAgentObserver, FakeMcpServer};
use llm::{ChatMessage, Context, LlmError, LlmModel, LlmResponse, ModelSettings, StreamingModelProvider};

use llm::testing::FakeLlmProvider;
use mcp_utils::client::McpServer;

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
                    TurnEvent::Started { .. } | TurnEvent::LlmCallStarted { .. } | TurnEvent::LlmCallEnded { .. }
                ) | AgentEvent::Tool(ToolEvent::ExecutionStarted { .. } | ToolEvent::DefinitionsUpdated { .. })
            )
        })
        .collect()
}

pub fn mcp_instructions(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

pub fn test_agent() -> TestAgentBuilder {
    TestAgentBuilder::new()
}

/// An ordered interaction with a test agent.
pub enum TestAgentStep {
    Send(Command),
    WaitFor(Box<dyn Fn(&AgentEvent) -> bool + Send>),
    /// Run an arbitrary side effect (e.g. releasing a paused LLM stream) between steps.
    Perform(Box<dyn FnOnce() + Send>),
}

impl TestAgentStep {
    pub fn send(command: Command) -> Self {
        Self::Send(command)
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self::send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text(text.into())] }))
    }

    pub fn cancel() -> Self {
        Self::send(Command::UserCommand(UserCommand::Cancel))
    }

    pub fn switch_model(provider: impl StreamingModelProvider + 'static) -> Self {
        Self::send(Command::AgentCommand(AgentCommand::SwitchModel(Box::new(provider))))
    }

    pub fn replace_conversation(messages: Vec<ChatMessage>) -> Self {
        Self::send(Command::AgentCommand(AgentCommand::ReplaceConversation(messages)))
    }

    pub fn perform(action: impl FnOnce() + Send + 'static) -> Self {
        Self::Perform(Box::new(action))
    }

    pub fn wait_for(predicate: impl Fn(&AgentEvent) -> bool + Send + 'static) -> Self {
        Self::WaitFor(Box::new(predicate))
    }

    pub fn wait_for_turn_end() -> Self {
        Self::wait_for(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. })))
    }

    pub fn wait_for_compaction_start() -> Self {
        Self::wait_for(|event| matches!(event, AgentEvent::Context(ContextEvent::CompactionStarted { .. })))
    }

    pub fn wait_for_retry(attempt: u32) -> Self {
        Self::wait_for(
            move |event| matches!(event, AgentEvent::Turn(TurnEvent::RetryScheduled { attempt: actual, .. }) if *actual == attempt),
        )
    }
}

/// A fluent sequence of commands and synchronization points for a test agent.
#[derive(Default)]
pub struct TestScenario {
    steps: Vec<TestAgentStep>,
}

impl TestScenario {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn send(mut self, command: Command) -> Self {
        self.steps.push(TestAgentStep::send(command));
        self
    }

    pub fn user_text(mut self, text: impl Into<String>) -> Self {
        self.steps.push(TestAgentStep::user_text(text));
        self
    }

    pub fn cancel(mut self) -> Self {
        self.steps.push(TestAgentStep::cancel());
        self
    }

    pub fn switch_model(mut self, provider: impl StreamingModelProvider + 'static) -> Self {
        self.steps.push(TestAgentStep::switch_model(provider));
        self
    }

    pub fn replace_conversation(mut self, messages: Vec<ChatMessage>) -> Self {
        self.steps.push(TestAgentStep::replace_conversation(messages));
        self
    }

    pub fn wait_for(mut self, predicate: impl Fn(&AgentEvent) -> bool + Send + 'static) -> Self {
        self.steps.push(TestAgentStep::wait_for(predicate));
        self
    }

    pub fn wait_for_turn_end(mut self) -> Self {
        self.steps.push(TestAgentStep::wait_for_turn_end());
        self
    }

    pub fn wait_for_compaction_start(mut self) -> Self {
        self.steps.push(TestAgentStep::wait_for_compaction_start());
        self
    }

    pub fn wait_for_retry(mut self, attempt: u32) -> Self {
        self.steps.push(TestAgentStep::wait_for_retry(attempt));
        self
    }

    /// Run an arbitrary side effect between scenario steps, e.g. releasing a
    /// paused LLM stream so a queued message can be injected mid-turn.
    pub fn perform(mut self, action: impl FnOnce() + Send + 'static) -> Self {
        self.steps.push(TestAgentStep::perform(action));
        self
    }
}

impl From<Vec<TestAgentStep>> for TestScenario {
    fn from(steps: Vec<TestAgentStep>) -> Self {
        Self { steps }
    }
}

/// Result of running a test agent, including messages and captured contexts.
pub struct TestAgentResult {
    pub messages: Vec<AgentEvent>,
    pub captured_contexts: Arc<Mutex<Vec<Context>>>,
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

struct ProviderTestConfig {
    responses: Vec<Vec<Result<LlmResponse, LlmError>>>,
    model: Option<LlmModel>,
    context_window: Option<u32>,
    pause: Option<(usize, usize, Arc<Notify>)>,
}

struct AgentTestConfig {
    context_window_override: Option<u32>,
    timeout: Option<Duration>,
    max_auto_continues: Option<u32>,
    retry_config: Option<RetryConfig>,
    observers: Vec<Box<dyn AgentObserver>>,
    mcp_servers: Option<Vec<McpServer>>,
    initial_messages: Vec<ChatMessage>,
    system_prompt: Option<Prompt>,
    compaction: Option<CompactionConfig>,
    model_settings: Option<ModelSettings>,
}

enum TestExecution {
    CommandsUntilTurnEnd(Vec<Command>),
    Scenario(TestScenario),
}

pub struct TestAgentBuilder {
    provider: ProviderTestConfig,
    agent: AgentTestConfig,
    execution: Option<TestExecution>,
}

impl Default for TestAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestAgentBuilder {
    pub fn new() -> Self {
        Self {
            provider: ProviderTestConfig { responses: Vec::new(), model: None, context_window: None, pause: None },
            agent: AgentTestConfig {
                context_window_override: None,
                timeout: None,
                max_auto_continues: None,
                retry_config: None,
                observers: Vec::new(),
                mcp_servers: Some(vec![fake_mcp("test", FakeMcpServer::new())]),
                initial_messages: Vec::new(),
                system_prompt: None,
                compaction: None,
                model_settings: None,
            },
            execution: None,
        }
    }

    pub fn commands(self, commands: Vec<Command>) -> Self {
        self.with_execution(TestExecution::CommandsUntilTurnEnd(commands))
    }

    pub fn scenario(self, scenario: impl Into<TestScenario>) -> Self {
        self.with_execution(TestExecution::Scenario(scenario.into()))
    }

    pub fn user_text(self, text: &str) -> Self {
        self.commands(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text(text)] })])
    }

    pub fn llm_responses(mut self, llm_responses: &[Vec<LlmResponse>]) -> Self {
        self.provider.responses = llm_responses.iter().map(|turn| turn.iter().cloned().map(Ok).collect()).collect();
        self
    }

    pub fn llm_result_responses(mut self, llm_responses: &[Vec<Result<LlmResponse, LlmError>>]) -> Self {
        self.provider.responses = Vec::from(llm_responses);
        self
    }

    pub fn model(mut self, model: LlmModel) -> Self {
        self.provider.model = Some(model);
        self
    }

    pub fn provider_context_window(mut self, window: Option<u32>) -> Self {
        self.provider.context_window = window;
        self
    }

    pub fn context_window_override(mut self, window: u32) -> Self {
        self.agent.context_window_override = Some(window);
        self
    }

    pub fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.agent.timeout = Some(timeout);
        self
    }

    pub fn max_auto_continues(mut self, max: u32) -> Self {
        self.agent.max_auto_continues = Some(max);
        self
    }

    pub fn retry_config(mut self, config: RetryConfig) -> Self {
        self.agent.retry_config = Some(config);
        self
    }

    /// Run without the default fake MCP server when the scenario does not exercise tools.
    pub fn without_mcp(mut self) -> Self {
        self.agent.mcp_servers = None;
        self
    }

    /// Replace the default fake MCP server with a scripted server.
    pub fn fake_mcp_server(mut self, name: &str, server: FakeMcpServer) -> Self {
        self.agent.mcp_servers = Some(vec![fake_mcp(name, server)]);
        self
    }

    /// Pre-populate the context with conversation history.
    pub fn messages(mut self, messages: Vec<ChatMessage>) -> Self {
        self.agent.initial_messages = messages;
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: Prompt) -> Self {
        self.agent.system_prompt = Some(prompt);
        self
    }

    /// Configure context compaction settings.
    pub fn compaction_config(mut self, config: CompactionConfig) -> Self {
        self.agent.compaction = Some(config);
        self
    }

    /// Set the model settings applied to every LLM call.
    pub fn model_settings(mut self, settings: ModelSettings) -> Self {
        self.agent.model_settings = Some(settings);
        self
    }

    /// Pause the fake LLM stream at `turn_index` / `chunk_index` until
    /// `release.notify_one()` is called. Used for deterministic timing tests.
    pub fn pause_turn_after(mut self, turn_index: usize, chunk_index: usize, release: Arc<Notify>) -> Self {
        self.provider.pause = Some((turn_index, chunk_index, release));
        self
    }

    /// Attach an observer of the test agent's event stream.
    pub fn observer(mut self, observer: Box<dyn AgentObserver>) -> Self {
        self.agent.observers.push(observer);
        self
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
        let Self { provider, agent: config, execution } = self;
        let mut llm = FakeLlmProvider::from_results(provider.responses).with_context_window(provider.context_window);
        if let Some(model) = provider.model {
            llm = llm.with_model(model);
        }
        if let Some((turn_index, chunk_index, release)) = provider.pause {
            llm = llm.pause_turn_after(turn_index, chunk_index, release);
        }
        let captured_contexts = llm.captured_contexts();

        let mut mcp_spawn = match config.mcp_servers {
            Some(servers) => Some(mcp("/workspace").with_servers(servers).spawn().await.map_err(AgentError::from)?),
            None => None,
        };

        let mut builder = agent(llm);
        if let Some(spawn) = &mut mcp_spawn {
            let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");
            builder = builder.tools(spawn.command_client(), snapshot.tool_definitions);
        }
        if let Some(timeout) = config.timeout {
            builder = builder.tool_timeout(timeout);
        }
        if let Some(max) = config.max_auto_continues {
            builder = builder.max_auto_continues(max);
        }
        if let Some(retry) = config.retry_config {
            builder = builder.retry(retry);
        } else {
            builder = builder.retry(RetryConfig::disabled());
        }
        if let Some(prompt) = config.system_prompt {
            builder = builder.system_prompt(prompt);
        }
        if let Some(compaction) = config.compaction {
            builder = builder.compaction(compaction);
        }
        if let Some(settings) = config.model_settings {
            builder = builder.model_settings(settings);
        }
        builder = builder.context_window(config.context_window_override);
        if !config.initial_messages.is_empty() {
            builder = builder.messages(config.initial_messages);
        }
        for observer in config.observers {
            builder = builder.observer(observer);
        }

        let steps = match execution.expect("test agent requires commands(), user_text(), or scenario()") {
            TestExecution::CommandsUntilTurnEnd(commands) => {
                assert!(!commands.is_empty(), "commands() requires at least one command");
                let mut steps = commands.into_iter().map(TestAgentStep::send).collect::<Vec<_>>();
                steps.push(TestAgentStep::wait_for_turn_end());
                steps
            }
            TestExecution::Scenario(scenario) => {
                assert!(!scenario.steps.is_empty(), "scenario() requires at least one step");
                scenario.steps
            }
        };
        let (tx, mut rx, handle) = builder.spawn().await?;
        let mut messages = Vec::new();

        for step in steps {
            match step {
                TestAgentStep::Send(command) => tx.send(command).await?,
                TestAgentStep::WaitFor(predicate) => loop {
                    let message = rx.recv().await.expect("agent event channel closed before scenario step matched");
                    let matched = predicate(&message);
                    messages.push(message);
                    if matched {
                        break;
                    }
                },
                TestAgentStep::Perform(action) => action(),
            }
        }
        drop(tx);

        handle.await_completion().await;

        Ok(TestAgentResult { messages, captured_contexts })
    }

    fn with_execution(mut self, execution: TestExecution) -> Self {
        assert!(self.execution.is_none(), "commands(), user_text(), and scenario() are mutually exclusive");
        self.execution = Some(execution);
        self
    }
}
