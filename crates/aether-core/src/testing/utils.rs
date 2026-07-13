use crate::events::{ToolEvent, TurnEvent};
use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use futures::future::join_all;

use crate::core::{RetryConfig, agent};
use crate::events::{AgentEvent, AgentObserver, Command, UserCommand};
use crate::mcp::mcp;
use crate::testing::fake_mcp::fake_mcp;
use crate::testing::{AgentTrace, FakeAgentObserver, FakeMcpServer};
use llm::{Context, LlmError, LlmModel, LlmResponse};

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

/// Result of running a test agent, including messages and captured contexts.
pub struct TestAgentResult {
    pub messages: Vec<AgentEvent>,
    pub captured_contexts: Arc<Mutex<Vec<Context>>>,
}

type CancelPredicate = Box<dyn Fn(&AgentEvent) -> bool + Send>;

pub struct TestAgentBuilder {
    messages: Vec<Command>,
    responses: Vec<Vec<Result<LlmResponse, LlmError>>>,
    model: Option<LlmModel>,
    context_window: Option<u32>,
    timeout: Option<Duration>,
    max_auto_continues: Option<u32>,
    retry_config: Option<RetryConfig>,
    observers: Vec<Box<dyn AgentObserver>>,
    cancel_when: Option<CancelPredicate>,
}

impl Default for TestAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestAgentBuilder {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            responses: Vec::new(),
            model: None,
            context_window: None,
            timeout: None,
            max_auto_continues: None,
            retry_config: None,
            observers: Vec::new(),
            cancel_when: None,
        }
    }

    pub fn user_messages(mut self, user_messages: Vec<Command>) -> Self {
        self.messages = user_messages;
        self
    }

    pub fn user_text(self, text: &str) -> Self {
        self.user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text(text)],
        })])
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

    pub fn context_window(mut self, window: u32) -> Self {
        self.context_window = Some(window);
        self
    }

    /// Sends `UserCommand::Cancel` as soon as a received event matches
    /// `predicate`, enabling deterministic mid-turn cancellation tests.
    pub fn cancel_when(mut self, predicate: impl Fn(&AgentEvent) -> bool + Send + 'static) -> Self {
        self.cancel_when = Some(Box::new(predicate));
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
        let mut llm = FakeLlmProvider::from_results(self.responses).with_context_window(self.context_window);
        if let Some(model) = self.model {
            llm = llm.with_model(model);
        }
        let captured_contexts = llm.captured_contexts();

        let mut spawn = mcp("/workspace").with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await?;
        let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");

        let mut builder = agent(llm).tools(spawn.command_tx, snapshot.tool_definitions);
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
        for observer in self.observers {
            builder = builder.observer(observer);
        }

        let (tx, mut rx, handle) = builder.spawn().await?;
        let futures: Vec<_> = self.messages.into_iter().map(|m| tx.send(m)).collect();

        join_all(futures).await;

        let mut command_tx = if self.cancel_when.is_some() {
            Some(tx)
        } else {
            drop(tx);
            None
        };
        let mut messages = Vec::new();
        while let Some(message) = rx.recv().await {
            messages.push(message.clone());
            if self.cancel_when.as_ref().is_some_and(|predicate| predicate(&message))
                && let Some(tx) = command_tx.take()
            {
                tx.send(Command::UserCommand(UserCommand::Cancel)).await?;
            }
            if matches!(message, AgentEvent::Turn(TurnEvent::Ended { .. })) {
                break;
            }
        }
        drop(command_tx);

        handle.await_completion().await;

        Ok(TestAgentResult { messages, captured_contexts })
    }
}
