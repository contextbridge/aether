use crate::events::{ToolEvent, TurnEvent};
use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

use crate::context::CompactionConfig;
use crate::core::{Prompt, RetryConfig, agent};
use crate::events::{AgentEvent, AgentObserver, Command, UserCommand};
use crate::mcp::mcp;
use crate::testing::fake_mcp::fake_mcp;
use crate::testing::{AgentTrace, FakeAgentObserver, FakeMcpServer};
use llm::{ChatMessage, Context, LlmError, LlmModel, LlmResponse, ModelSettings};

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
}

impl TestAgentStep {
    pub fn send(command: Command) -> Self {
        Self::Send(command)
    }

    pub fn wait_for(predicate: impl Fn(&AgentEvent) -> bool + Send + 'static) -> Self {
        Self::WaitFor(Box::new(predicate))
    }
}

/// Result of running a test agent, including messages and captured contexts.
pub struct TestAgentResult {
    pub messages: Vec<AgentEvent>,
    pub captured_contexts: Arc<Mutex<Vec<Context>>>,
}

pub struct TestAgentBuilder {
    commands: Vec<Command>,
    scenario: Option<Vec<TestAgentStep>>,
    responses: Vec<Vec<Result<LlmResponse, LlmError>>>,
    model: Option<LlmModel>,
    provider_context_window: Option<u32>,
    context_window_override: Option<u32>,
    timeout: Option<Duration>,
    max_auto_continues: Option<u32>,
    retry_config: Option<RetryConfig>,
    observers: Vec<Box<dyn AgentObserver>>,
    include_fake_mcp: bool,
    initial_messages: Vec<ChatMessage>,
    system_prompt: Option<Prompt>,
    compaction_config: Option<CompactionConfig>,
    model_settings: Option<ModelSettings>,
    pause: Option<(usize, usize, Arc<Notify>)>,
}

impl Default for TestAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestAgentBuilder {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            scenario: None,
            responses: Vec::new(),
            model: None,
            provider_context_window: None,
            context_window_override: None,
            timeout: None,
            max_auto_continues: None,
            retry_config: None,
            observers: Vec::new(),
            include_fake_mcp: true,
            initial_messages: Vec::new(),
            system_prompt: None,
            compaction_config: None,
            model_settings: None,
            pause: None,
        }
    }

    pub fn commands(mut self, commands: Vec<Command>) -> Self {
        self.commands = commands;
        self
    }

    pub fn scenario(mut self, steps: Vec<TestAgentStep>) -> Self {
        self.scenario = Some(steps);
        self
    }

    pub fn user_text(self, text: &str) -> Self {
        self.commands(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text(text)] })])
    }

    pub fn llm_responses(mut self, llm_responses: &[Vec<LlmResponse>]) -> Self {
        self.responses = llm_responses.iter().map(|turn| turn.iter().cloned().map(Ok).collect()).collect();
        self
    }

    pub fn llm_result_responses(mut self, llm_responses: &[Vec<Result<LlmResponse, LlmError>>]) -> Self {
        self.responses = Vec::from(llm_responses);
        self
    }

    pub fn model(mut self, model: LlmModel) -> Self {
        self.model = Some(model);
        self
    }

    pub fn provider_context_window(mut self, window: Option<u32>) -> Self {
        self.provider_context_window = window;
        self
    }

    pub fn context_window_override(mut self, window: u32) -> Self {
        self.context_window_override = Some(window);
        self
    }

    pub fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn max_auto_continues(mut self, max: u32) -> Self {
        self.max_auto_continues = Some(max);
        self
    }

    pub fn retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = Some(config);
        self
    }

    /// Run without the default fake MCP server when the scenario does not exercise tools.
    pub fn without_mcp(mut self) -> Self {
        self.include_fake_mcp = false;
        self
    }

    /// Pre-populate the context with conversation history.
    pub fn messages(mut self, messages: Vec<ChatMessage>) -> Self {
        self.initial_messages = messages;
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: Prompt) -> Self {
        self.system_prompt = Some(prompt);
        self
    }

    /// Configure context compaction settings.
    pub fn compaction_config(mut self, config: CompactionConfig) -> Self {
        self.compaction_config = Some(config);
        self
    }

    /// Set the model settings applied to every LLM call.
    pub fn model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Pause the fake LLM stream at `turn_index` / `chunk_index` until
    /// `release.notify_one()` is called. Used for deterministic timing tests.
    pub fn pause_turn_after(mut self, turn_index: usize, chunk_index: usize, release: Arc<Notify>) -> Self {
        self.pause = Some((turn_index, chunk_index, release));
        self
    }

    /// Attach an observer of the test agent's event stream.
    pub fn observer(mut self, observer: Box<dyn AgentObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    pub async fn run(self) -> Result<Vec<AgentEvent>, Box<dyn Error>> {
        let result = self.run_with_context().await?;
        Ok(result.messages)
    }

    /// Runs the test agent with a recording observer attached and returns the
    /// full event trace, including internal events.
    pub async fn run_trace(self) -> Result<AgentTrace, Box<dyn Error>> {
        let observer = FakeAgentObserver::new();
        let events = observer.events();
        self.observer(Box::new(observer)).run().await?;
        Ok(AgentTrace::from_observer_events(&events))
    }

    /// Runs the test agent and returns both messages and captured contexts.
    ///
    /// Use this when you need to verify what context was passed to the LLM,
    /// for example when testing that file attachments are properly formatted.
    pub async fn run_with_context(self) -> Result<TestAgentResult, Box<dyn Error>> {
        let mut llm = FakeLlmProvider::from_results(self.responses).with_context_window(self.provider_context_window);
        if let Some(model) = self.model {
            llm = llm.with_model(model);
        }
        if let Some((turn_index, chunk_index, release)) = self.pause {
            llm = llm.pause_turn_after(turn_index, chunk_index, release);
        }
        let captured_contexts = llm.captured_contexts();

        let mut mcp_spawn = if self.include_fake_mcp {
            Some(mcp("/workspace").with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await?)
        } else {
            None
        };

        let mut builder = agent(llm);
        if let Some(spawn) = &mut mcp_spawn {
            let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");
            builder = builder.tools(spawn.command_tx.clone(), snapshot.tool_definitions);
        }
        if let Some(timeout) = self.timeout {
            builder = builder.tool_timeout(timeout);
        }
        if let Some(max) = self.max_auto_continues {
            builder = builder.max_auto_continues(max);
        }
        if let Some(retry) = self.retry_config {
            builder = builder.retry(retry);
        } else {
            builder = builder.retry(RetryConfig::disabled());
        }
        if let Some(prompt) = self.system_prompt {
            builder = builder.system_prompt(prompt);
        }
        if let Some(compaction) = self.compaction_config {
            builder = builder.compaction(compaction);
        }
        if let Some(settings) = self.model_settings {
            builder = builder.model_settings(settings);
        }
        builder = builder.context_window(self.context_window_override);
        if !self.initial_messages.is_empty() {
            builder = builder.messages(self.initial_messages);
        }
        for observer in self.observers {
            builder = builder.observer(observer);
        }

        let (tx, mut rx, handle) = builder.spawn().await?;
        let steps = self.scenario.unwrap_or_else(|| {
            let mut steps = self.commands.into_iter().map(TestAgentStep::send).collect::<Vec<_>>();
            steps.push(TestAgentStep::wait_for(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. }))));
            steps
        });
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
            }
        }
        drop(tx);

        handle.await_completion().await;

        Ok(TestAgentResult { messages, captured_contexts })
    }
}
