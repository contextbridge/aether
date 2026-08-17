use crate::context::{CompactionConfig, CompactionError, CompactionResult, Compactor, TokenTracker};
use crate::core::PromptCache;
use crate::core::prompt_cache_key::derive_prompt_cache_key;
use crate::core::queued_input::QueuedInput;
pub use crate::core::retry_config::RetryConfig;
use crate::core::tool_execution::{ToolAbortPolicy, ToolExecutionUpdate, ToolExecutions};
use crate::events::{
    AgentCommand, AgentEvent, AgentObserver, Command, CompactionOutcome, ContextEvent, ContextUsage, LlmCallOutcome,
    LlmCallPurpose, ModelEvent, StreamState, TaskOutcome, ToolEvent, TraceContext, TurnEvent, TurnOutcome, UserCommand,
};
use crate::mcp::McpHandle;
use futures::Stream;
use llm::types::IsoString;
use llm::{
    AssistantReasoning, ChatMessage, Context, EncryptedReasoningContent, LlmError, LlmResponse, StopReason,
    StreamingModelProvider, TokenUsage, ToolCallError, ToolCallRequest, ToolCallResult,
};
use mcp_utils::client::{CallToolError, CallToolOptions, ToolCallEvent};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tokio_stream::StreamMap;
use tokio_stream::wrappers::ReceiverStream;

/// Internal event type for merging LLM and tool result streams
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum StreamEvent {
    LlmRequestStarted { attempt: u32 },
    Llm(Result<LlmResponse, LlmError>),
    ToolExecution(ToolCallEvent),
    Command(Command),
    InputClosed,
    Compaction(Result<CompactionResult, CompactionError>),
}

type EventStream = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;

/// Keys for the merged stream map. Tool-call IDs come from providers, so the
/// typed key keeps them from colliding with reserved streams.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StreamKey {
    Input,
    Llm,
    Compaction,
    Tool(String),
}

pub(crate) struct AgentConfig {
    pub llm: Arc<dyn StreamingModelProvider>,
    pub context: Context,
    pub mcp: Option<McpHandle>,
    pub tool_timeout: Duration,
    pub compaction_config: Option<CompactionConfig>,
    pub auto_continue: AutoContinue,
    pub retry_config: RetryConfig,
    pub context_window: Option<u32>,
    pub prompt_cache: PromptCache,
    pub observers: Vec<Box<dyn AgentObserver>>,
}

pub struct Agent {
    llm: Arc<dyn StreamingModelProvider>,
    context: Context,
    mcp: Option<McpHandle>,
    message_tx: mpsc::Sender<AgentEvent>,
    observers: Vec<Box<dyn AgentObserver>>,
    streams: StreamMap<StreamKey, EventStream>,
    tool_timeout: Duration,
    token_tracker: TokenTracker,
    compaction_config: Option<CompactionConfig>,
    auto_continue: AutoContinue,
    retry_config: RetryConfig,
    tool_executions: ToolExecutions,
    pending_inputs: VecDeque<QueuedInput>,
    queued_inputs: VecDeque<QueuedInput>,
    context_window: Option<u32>,
    prompt_cache: PromptCache,
    turn_active: bool,
    llm_call_active: bool,
}

impl Agent {
    pub(crate) fn new(
        config: AgentConfig,
        command_rx: mpsc::Receiver<Command>,
        message_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        let mut streams: StreamMap<StreamKey, EventStream> = StreamMap::new();
        let input_stream = ReceiverStream::new(command_rx)
            .map(StreamEvent::Command)
            .chain(futures::stream::once(async { StreamEvent::InputClosed }));
        streams.insert(StreamKey::Input, Box::pin(input_stream));

        let context_limit = config.context_window.or_else(|| config.llm.context_window());

        Self {
            llm: config.llm,
            context: config.context,
            mcp: config.mcp,
            message_tx,
            observers: config.observers,
            streams,
            tool_timeout: config.tool_timeout,
            token_tracker: TokenTracker::new(context_limit),
            compaction_config: config.compaction_config,
            auto_continue: config.auto_continue,
            retry_config: config.retry_config,
            tool_executions: ToolExecutions::default(),
            pending_inputs: VecDeque::new(),
            queued_inputs: VecDeque::new(),
            context_window: config.context_window,
            prompt_cache: config.prompt_cache,
            turn_active: false,
            llm_call_active: false,
        }
    }

    pub fn current_model_display_name(&self) -> String {
        self.llm.display_name()
    }

    /// Get a reference to the token tracker
    pub fn token_tracker(&self) -> &TokenTracker {
        &self.token_tracker
    }

    pub async fn run(mut self) {
        let mut state = IterationState::default();
        let mut input_closed = false;
        self.emit_tool_definitions().await;

        while let Some((stream_key, event)) = self.streams.next().await {
            match event {
                StreamEvent::Command(Command::UserCommand(UserCommand::Cancel)) => {
                    self.on_user_cancel(&mut state).await;
                }

                StreamEvent::Command(Command::UserCommand(UserCommand::ClearContext)) => {
                    self.on_user_clear_context(&mut state).await;
                }

                StreamEvent::Command(Command::UserCommand(UserCommand::Text { content })) => {
                    if self.is_busy() {
                        self.queued_inputs.push_back(QueuedInput::User(content));
                    } else {
                        self.begin_turn(QueuedInput::User(content), &mut state).await;
                    }
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::SwitchModel(new_provider))) => {
                    self.on_switch_model(new_provider).await;
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::UpdateTools(tools))) => {
                    self.context.set_tools(tools);
                    self.emit_tool_definitions().await;
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::UpdateMcpInstructions { server, body })) => {
                    self.on_update_instruction(server, body).await;
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::SetReasoningEffort(effort))) => {
                    self.context.set_reasoning_effort(effort);
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::ReplaceConversation(messages))) => {
                    self.on_replace_conversation(messages, &mut state).await;
                }

                StreamEvent::InputClosed => {
                    input_closed = true;
                }

                StreamEvent::LlmRequestStarted { attempt } => {
                    self.begin_chat_call(attempt).await;
                }

                StreamEvent::Llm(llm_event) => {
                    self.on_llm_event(llm_event, &mut state).await;
                }

                StreamEvent::ToolExecution(tool_event) => {
                    let StreamKey::Tool(tool_id) = stream_key else {
                        unreachable!("tool events must come from a tool stream")
                    };
                    self.on_tool_execution_event(tool_id, tool_event, &mut state).await;
                }

                StreamEvent::Compaction(result) => {
                    self.on_compaction_complete(result).await;
                }
            }

            if state.is_complete(self.tool_executions.has_foreground())
                && let Some(id) = state.current_message_id.take()
            {
                let iteration = std::mem::take(&mut state);
                self.on_iteration_complete(id, iteration).await;
            }

            if input_closed && !self.turn_active && !self.is_busy() && self.tool_executions.is_empty() {
                self.abort_in_flight_work(ToolAbortPolicy::CancelAll).await;
                break;
            }
        }

        tracing::debug!("Agent task shutting down - input channel closed");
    }

    async fn on_iteration_complete(&mut self, id: String, iteration: IterationState) {
        let IterationState {
            message_content,
            reasoning_summary_text,
            encrypted_reasoning,
            completed_tool_calls,
            stop_reason,
            ..
        } = iteration;
        let has_tool_calls = !completed_tool_calls.is_empty();
        let has_content = !message_content.is_empty() || has_tool_calls;
        let should_auto_continue = self.auto_continue.should_continue(stop_reason.as_ref());

        if has_content {
            let reasoning = AssistantReasoning::from_parts(reasoning_summary_text.clone(), encrypted_reasoning);
            self.context.push_assistant_turn(&message_content, reasoning, completed_tool_calls);

            self.emit(AgentEvent::text(&id, &message_content, StreamState::Complete)).await;

            if !reasoning_summary_text.is_empty() {
                self.emit(AgentEvent::thought(&id, &reasoning_summary_text, StreamState::Complete)).await;
            }
        }

        let has_queued_input = !self.queued_inputs.is_empty();
        if has_queued_input || has_tool_calls {
            self.auto_continue.reset();
            self.start_next_turn().await;
        } else if should_auto_continue {
            self.auto_continue.advance();
            tracing::info!(
                "LLM stopped with {:?}, auto-continuing (attempt {}/{})",
                stop_reason,
                self.auto_continue.count,
                self.auto_continue.max
            );

            self.emit(AgentEvent::Turn(TurnEvent::AutoContinue {
                attempt: self.auto_continue.count,
                max_attempts: self.auto_continue.max,
            }))
            .await;

            self.inject_continuation_prompt(&message_content, stop_reason.as_ref());
            self.start_next_turn().await;
        } else {
            tracing::debug!("LLM completed turn with stop reason: {:?}", stop_reason);
            self.auto_continue.reset();
            self.finish_turn(TurnOutcome::Completed).await;
        }
    }

    async fn start_next_turn(&mut self) {
        debug_assert!(self.pending_inputs.is_empty());
        self.pending_inputs.append(&mut self.queued_inputs);
        if self.compaction_needed() {
            self.begin_compaction().await;
        } else {
            self.start_chat_turn().await;
        }
    }

    async fn start_chat_turn(&mut self) {
        self.commit_pending_inputs().await;
        self.start_llm_stream(None, 0).await;
    }

    async fn on_user_cancel(&mut self, state: &mut IterationState) {
        self.abort_in_flight_work(ToolAbortPolicy::PreserveBackgroundAcknowledgements).await;
        self.commit_pending_inputs().await;
        self.queued_inputs.retain(|input| matches!(input, QueuedInput::TaskOutcome(_)));
        self.commit_queued_inputs().await;
        *state = IterationState::default();
        self.finish_turn(TurnOutcome::Cancelled).await;
    }

    async fn discard_in_flight_work(&mut self, state: &mut IterationState) {
        self.abort_in_flight_work(ToolAbortPolicy::CancelAll).await;
        self.pending_inputs.clear();
        self.queued_inputs.clear();
        self.auto_continue.reset();
        *state = IterationState::default();
    }

    async fn on_user_clear_context(&mut self, state: &mut IterationState) {
        self.discard_in_flight_work(state).await;
        self.context.clear_conversation();
        self.token_tracker.reset_current_usage();
        self.emit(AgentEvent::Context(ContextEvent::Cleared)).await;
        self.finish_turn(TurnOutcome::Cancelled).await;
    }

    async fn on_replace_conversation(&mut self, messages: Vec<ChatMessage>, state: &mut IterationState) {
        self.discard_in_flight_work(state).await;
        self.context.replace_conversation(messages);
        self.emit(self.context_usage_message()).await;
        self.finish_turn(TurnOutcome::Cancelled).await;
    }

    async fn begin_turn(&mut self, input: QueuedInput, state: &mut IterationState) {
        *state = IterationState::default();
        self.auto_continue.reset();
        self.turn_active = true;
        let content = input.content_blocks();
        self.emit(AgentEvent::Turn(TurnEvent::Started { content })).await;
        self.queued_inputs.push_back(input);
        self.start_next_turn().await;
    }

    async fn enqueue_task_outcome(&mut self, outcome: TaskOutcome, state: &mut IterationState) {
        let input = QueuedInput::TaskOutcome(Box::new(outcome));
        if self.is_busy() {
            self.queued_inputs.push_back(input);
        } else {
            self.begin_turn(input, state).await;
        }
    }

    async fn on_update_instruction(&mut self, server: String, body: Option<String>) {
        self.prompt_cache.update_mcp_instruction(server, body);
        match self.prompt_cache.render().await {
            Ok(content) => self.context.set_system_content(content),
            Err(e) => tracing::warn!("Failed to rebuild system prompt after instructions update: {e}"),
        }
    }

    async fn on_switch_model(&mut self, new_provider: Box<dyn StreamingModelProvider>) {
        let previous = self.llm.display_name();
        let new_context_limit = self.context_window.or_else(|| new_provider.context_window());
        self.llm = Arc::from(new_provider);
        self.token_tracker.reset_current_usage();
        self.token_tracker.set_context_limit(new_context_limit);
        let new = self.llm.display_name();
        self.emit(AgentEvent::Model(ModelEvent::Switched { previous, new })).await;

        self.emit(self.context_usage_message()).await;
    }

    async fn start_llm_stream(&mut self, delay: Option<Duration>, attempt: u32) {
        self.refresh_prompt_cache_key();
        self.streams.remove(&StreamKey::Llm);
        let stream: EventStream = match delay {
            None => {
                self.begin_chat_call(attempt).await;
                Box::pin(self.llm.stream_response(&self.context).map(StreamEvent::Llm))
            }
            Some(delay) => {
                self.emit(AgentEvent::Turn(TurnEvent::RetryScheduled {
                    purpose: LlmCallPurpose::Chat,
                    attempt,
                    max_attempts: self.retry_config.max_attempts,
                    delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                }))
                .await;
                let llm = Arc::clone(&self.llm);
                let context = self.context.clone();
                Box::pin(async_stream::stream! {
                    sleep(delay).await;
                    yield StreamEvent::LlmRequestStarted { attempt };
                    let mut inner = llm.stream_response(&context);
                    while let Some(item) = inner.next().await {
                        yield StreamEvent::Llm(item);
                    }
                })
            }
        };
        self.streams.insert(StreamKey::Llm, stream);
    }

    async fn on_llm_error(&mut self, error: LlmError, state: &mut IterationState) {
        let will_retry = error.is_retryable() && state.retry_attempt < self.retry_config.max_attempts;
        let error_message = error.to_string();
        self.finish_chat_call(LlmCallOutcome::Failed { error: error_message.clone(), will_retry }).await;

        if !will_retry {
            self.finish_turn(TurnOutcome::Failed { error: error_message }).await;
            return;
        }

        state.retry_attempt += 1;
        let delay = self.retry_config.compute_delay(state.retry_attempt);

        tracing::warn!(
            attempt = state.retry_attempt,
            max_attempts = self.retry_config.max_attempts,
            delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            error = %error,
            "Retrying LLM request after transient failure"
        );

        self.tool_executions.retire_foreground();
        self.start_llm_stream(Some(delay), state.retry_attempt).await;
    }

    fn is_busy(&self) -> bool {
        self.streams.contains_key(&StreamKey::Llm)
            || self.streams.contains_key(&StreamKey::Compaction)
            || self.tool_executions.has_foreground()
    }

    async fn abort_in_flight_work(&mut self, tool_policy: ToolAbortPolicy) {
        if self.llm_call_active {
            self.finish_chat_call(LlmCallOutcome::Cancelled).await;
        }
        if self.streams.remove(&StreamKey::Compaction).is_some() {
            self.emit(AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: LlmCallPurpose::Compaction,
                outcome: LlmCallOutcome::Cancelled,
            }))
            .await;
            self.emit(AgentEvent::Context(ContextEvent::CompactionEnded { outcome: CompactionOutcome::Cancelled }))
                .await;
        }
        self.streams.remove(&StreamKey::Llm);
        for tool_id in self.tool_executions.abort(&tool_policy) {
            self.streams.remove(&StreamKey::Tool(tool_id));
        }
    }

    /// Inject a continuation prompt when the LLM stops due to a resumable reason.
    fn inject_continuation_prompt(&mut self, previous_response: &str, stop_reason: Option<&StopReason>) {
        if !previous_response.is_empty() {
            self.context.add_message(ChatMessage::Assistant {
                content: previous_response.to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: Vec::new(),
            });
        }

        let reason = stop_reason.map_or_else(|| "Unknown".to_string(), |reason| format!("{reason:?}"));

        self.context.add_message(ChatMessage::User {
            content: vec![llm::ContentBlock::text(format!(
                "<system-notification>The LLM API stopped with reason '{reason}'. Continue from where you left off and finish your task.</system-notification>"
            ))],
            timestamp: IsoString::now(),
        });
    }

    async fn on_llm_event(&mut self, result: Result<LlmResponse, LlmError>, state: &mut IterationState) {
        use LlmResponse::{
            Done, EncryptedReasoning, Error, Reasoning, Start, Text, ToolRequestArg, ToolRequestComplete,
            ToolRequestStart, Usage,
        };

        let response = match result {
            Ok(response) => response,
            Err(e) => {
                self.on_llm_error(e, state).await;
                return;
            }
        };

        match response {
            Start { message_id } => {
                state.on_llm_start(message_id);
            }

            Text { chunk } => {
                self.handle_llm_text(chunk, state).await;
            }

            Reasoning { chunk } => {
                state.reasoning_summary_text.push_str(&chunk);
                if let Some(id) = state.current_message_id.clone() {
                    self.emit(AgentEvent::thought(&id, &chunk, StreamState::Partial)).await;
                }
            }

            EncryptedReasoning { id, content } => {
                if let Some(model) = self.llm.model() {
                    state.encrypted_reasoning = Some(EncryptedReasoningContent { id, model, content });
                }
            }

            ToolRequestStart { id, name } => {
                let request = ToolCallRequest { id, name, arguments: String::new() };
                self.emit(AgentEvent::Tool(ToolEvent::Call { request })).await;
            }

            ToolRequestArg { id, chunk } => {
                self.emit(AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: id, chunk })).await;
            }

            ToolRequestComplete { tool_call } => {
                self.handle_tool_completion(tool_call).await;
            }

            Done { stop_reason } => {
                state.llm_done = true;
                state.stop_reason = stop_reason;
                self.finish_chat_call(LlmCallOutcome::Completed {
                    stop_reason: state.stop_reason.clone(),
                    usage: state.call_usage.take(),
                })
                .await;
            }

            Error { message } => {
                self.finish_chat_call(LlmCallOutcome::Failed { error: message.clone(), will_retry: false }).await;
                self.finish_turn(TurnOutcome::Failed { error: message }).await;
            }

            Usage { tokens: sample } => {
                self.handle_llm_usage(sample, state).await;
            }
        }
    }

    async fn handle_llm_text(&mut self, chunk: String, state: &mut IterationState) {
        state.message_content.push_str(&chunk);

        if let Some(id) = state.current_message_id.clone() {
            self.emit(AgentEvent::text(&id, &chunk, StreamState::Partial)).await;
        }
    }

    async fn handle_tool_completion(&mut self, tool_call: ToolCallRequest) {
        let cancel = self.tool_executions.start(tool_call.clone());

        let tool_id = tool_call.id.clone();
        tracing::debug!("Tool execution started: {} ({})", tool_call.name, tool_id);
        self.emit(AgentEvent::Tool(ToolEvent::ExecutionStarted {
            tool_id: tool_id.clone(),
            tool_name: tool_call.name.clone(),
        }))
        .await;

        let Some(mcp) = self.mcp.clone() else {
            let stream = futures::stream::once(async {
                StreamEvent::ToolExecution(ToolCallEvent::Complete(Err(CallToolError::Unavailable {
                    message: "MCP runtime is not available".to_string(),
                })))
            });
            self.streams.insert(StreamKey::Tool(tool_id), Box::pin(stream));
            return;
        };

        let trace_context = self.observers.iter().find_map(|observer| observer.tool_trace_context(&tool_id));
        let options = CallToolOptions {
            timeout: self.tool_timeout,
            meta: trace_context.as_ref().map(TraceContext::to_meta),
            cancel,
        };
        let stream =
            mcp.call_model_visible(tool_call.name, &tool_call.arguments, options).map(StreamEvent::ToolExecution);
        self.streams.insert(StreamKey::Tool(tool_id), Box::pin(stream));
    }

    async fn handle_llm_usage(&mut self, sample: TokenUsage, state: &mut IterationState) {
        state.call_usage = Some(sample);
        self.token_tracker.record_usage(sample);
        let ratio_pct = self.token_tracker.usage_ratio().map(|r| r * 100.0);
        let remaining = self.token_tracker.tokens_remaining();
        tracing::debug!(?sample, ?ratio_pct, ?remaining, "Token usage");

        self.emit(self.context_usage_message()).await;
    }

    fn context_usage_message(&self) -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated { usage: ContextUsage::from(&self.token_tracker) })
    }

    fn compaction_needed(&self) -> bool {
        self.compaction_config.as_ref().is_some_and(|config| {
            self.token_tracker.needs_compaction(self.context.estimated_token_count(), config.threshold)
        })
    }

    async fn begin_compaction(&mut self) {
        tracing::info!("Starting context compaction - {} messages", self.context.message_count());
        self.emit(AgentEvent::Context(ContextEvent::CompactionStarted { message_count: self.context.message_count() }))
            .await;
        self.emit(self.llm_call_started(LlmCallPurpose::Compaction, 0)).await;

        let compactor = Compactor::new(self.llm.clone());
        let context = self.context.clone();
        let stream: EventStream =
            Box::pin(futures::stream::once(async move { StreamEvent::Compaction(compactor.compact(context).await) }));
        self.streams.insert(StreamKey::Compaction, stream);
    }

    async fn on_compaction_complete(&mut self, result: Result<CompactionResult, CompactionError>) {
        let outcome = match &result {
            Ok(result) => LlmCallOutcome::Completed { stop_reason: None, usage: result.usage },
            Err(e) => LlmCallOutcome::Failed { error: e.to_string(), will_retry: false },
        };
        self.emit(AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose: LlmCallPurpose::Compaction, outcome })).await;

        match result {
            Ok(result) => {
                tracing::info!("Context compacted: {} messages removed", result.messages_removed);
                self.context = self.context.with_compacted_summary(&result.summary);
                self.token_tracker.reset_current_usage();
                self.emit(AgentEvent::Context(ContextEvent::CompactionResult {
                    summary: result.summary,
                    messages_removed: result.messages_removed,
                }))
                .await;
                self.emit(AgentEvent::Context(ContextEvent::CompactionEnded { outcome: CompactionOutcome::Completed }))
                    .await;
            }
            Err(e) => {
                tracing::warn!("Context compaction failed: {e}");
                self.emit(AgentEvent::Context(ContextEvent::CompactionEnded {
                    outcome: CompactionOutcome::Failed { error: e.to_string() },
                }))
                .await;
            }
        }

        self.start_chat_turn().await;
    }

    async fn on_tool_execution_event(&mut self, tool_id: String, event: ToolCallEvent, state: &mut IterationState) {
        match self.tool_executions.on_event(&tool_id, event) {
            ToolExecutionUpdate::Event(event) => {
                self.emit(AgentEvent::Tool(event)).await;
            }
            ToolExecutionUpdate::Completed { result, event } => {
                self.streams.remove(&StreamKey::Tool(tool_id));
                state.completed_tool_calls.push(result);
                self.emit(AgentEvent::Tool(event)).await;
            }
            ToolExecutionUpdate::TaskCreated { result, event } => {
                state.completed_tool_calls.push(Ok(result));
                self.emit(AgentEvent::Tool(event)).await;
            }
            ToolExecutionUpdate::TaskCompleted(outcome) => {
                self.streams.remove(&StreamKey::Tool(tool_id));
                self.enqueue_task_outcome(outcome, state).await;
            }
            ToolExecutionUpdate::TaskCancelled(outcome) => {
                self.streams.remove(&StreamKey::Tool(tool_id));
                self.record_task_outcome(outcome).await;
            }
            ToolExecutionUpdate::Retired => {
                self.streams.remove(&StreamKey::Tool(tool_id));
            }
            ToolExecutionUpdate::Ignored => {
                tracing::debug!(%tool_id, "Ignoring unexpected tool execution event");
            }
        }
    }

    async fn record_task_outcome(&mut self, outcome: TaskOutcome) {
        self.context.add_message(outcome.context_message());
        self.emit(AgentEvent::Tool(outcome.into())).await;
    }

    fn refresh_prompt_cache_key(&mut self) {
        let key = derive_prompt_cache_key(self.llm.as_ref(), &self.context);
        self.context.set_prompt_cache_key(Some(key));
    }

    async fn commit_pending_inputs(&mut self) {
        let inputs = std::mem::take(&mut self.pending_inputs);
        self.commit_inputs(inputs).await;
    }

    async fn commit_queued_inputs(&mut self) {
        let inputs = std::mem::take(&mut self.queued_inputs);
        self.commit_inputs(inputs).await;
    }

    async fn commit_inputs(&mut self, inputs: VecDeque<QueuedInput>) {
        let mut user_content = Vec::new();
        for input in inputs {
            match input {
                QueuedInput::User(content) => user_content.extend(content),
                QueuedInput::TaskOutcome(outcome) => {
                    self.commit_user_content(&mut user_content);
                    self.record_task_outcome(*outcome).await;
                }
            }
        }
        self.commit_user_content(&mut user_content);
    }

    fn commit_user_content(&mut self, content: &mut Vec<llm::ContentBlock>) {
        if !content.is_empty() {
            self.context
                .add_message(ChatMessage::User { content: std::mem::take(content), timestamp: IsoString::now() });
        }
    }

    async fn emit_tool_definitions(&mut self) {
        let tools = self.context.tools().clone();
        if !tools.is_empty() {
            self.emit(AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools })).await;
        }
    }

    async fn emit(&mut self, message: AgentEvent) {
        for observer in &mut self.observers {
            observer.on_event(&message);
        }

        if let Err(e) = self.message_tx.send(message).await {
            tracing::warn!("Failed to send agent message: {e:?}");
        }
    }

    async fn finish_turn(&mut self, outcome: TurnOutcome) {
        if std::mem::take(&mut self.turn_active) {
            self.emit(AgentEvent::turn_ended(outcome)).await;
        }
    }

    async fn begin_chat_call(&mut self, attempt: u32) {
        self.llm_call_active = true;
        self.emit(self.llm_call_started(LlmCallPurpose::Chat, attempt)).await;
    }

    async fn finish_chat_call(&mut self, outcome: LlmCallOutcome) {
        if std::mem::take(&mut self.llm_call_active) {
            self.emit(AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose: LlmCallPurpose::Chat, outcome })).await;
        }
    }

    fn llm_call_started(&self, purpose: LlmCallPurpose, attempt: u32) -> AgentEvent {
        let model = self.llm.model();
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose,
            provider: model.as_ref().map(|m| m.provider().to_string()),
            model: model.as_ref().map(|m| m.model_id().into_owned()),
            pricing: model.and_then(|m| m.pricing()),
            display_name: self.llm.display_name(),
            attempt,
            max_attempts: self.retry_config.max_attempts,
        })
    }
}

pub(crate) struct AutoContinue {
    max: u32,
    count: u32,
}

impl AutoContinue {
    pub(crate) fn new(max: u32) -> Self {
        Self { max, count: 0 }
    }

    fn reset(&mut self) {
        self.count = 0;
    }

    fn should_continue(&self, stop_reason: Option<&StopReason>) -> bool {
        matches!(stop_reason, Some(StopReason::Length)) && self.count < self.max
    }

    fn advance(&mut self) {
        self.count += 1;
    }
}

#[derive(Debug, Default)]
struct IterationState {
    current_message_id: Option<String>,
    message_content: String,
    reasoning_summary_text: String,
    encrypted_reasoning: Option<EncryptedReasoningContent>,
    completed_tool_calls: Vec<Result<ToolCallResult, ToolCallError>>,
    llm_done: bool,
    stop_reason: Option<StopReason>,
    retry_attempt: u32,
    call_usage: Option<TokenUsage>,
}

impl IterationState {
    fn on_llm_start(&mut self, message_id: String) {
        self.current_message_id = Some(message_id);
        self.message_content.clear();
        self.reasoning_summary_text.clear();
        self.encrypted_reasoning = None;
        self.stop_reason = None;
        self.call_usage = None;
    }

    fn is_complete(&self, has_foreground_tools: bool) -> bool {
        self.llm_done && !has_foreground_tools
    }
}
