use crate::context::{CompactionConfig, Compactor, TokenTracker};
pub use crate::core::retry_config::RetryConfig;
use crate::events::{AgentMessage, UserMessage};
use crate::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use futures::Stream;
use llm::types::IsoString;
use llm::{
    AssistantReasoning, ChatMessage, Context, EncryptedReasoningContent, LlmError, LlmResponse, StopReason,
    StreamingModelProvider, TokenUsage, ToolCallError, ToolCallRequest, ToolCallResult,
};
use std::collections::{HashMap, HashSet, VecDeque};
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
enum StreamEvent {
    Llm(Result<LlmResponse, LlmError>),
    ToolExecution(ToolExecutionEvent),
    UserMessage(UserMessage),
}

type EventStream = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;

const USER_STREAM_KEY: &str = "user";
const LLM_STREAM_KEY: &str = "llm";

pub(crate) struct AgentConfig {
    pub llm: Arc<dyn StreamingModelProvider>,
    pub context: Context,
    pub mcp_command_tx: Option<mpsc::Sender<McpCommand>>,
    pub tool_timeout: Duration,
    pub compaction_config: Option<CompactionConfig>,
    pub auto_continue: AutoContinue,
    pub retry_config: RetryConfig,
    pub context_window: Option<u32>,
}

pub struct Agent {
    llm: Arc<dyn StreamingModelProvider>,
    context: Context,
    mcp_command_tx: Option<mpsc::Sender<McpCommand>>,
    message_tx: mpsc::Sender<AgentMessage>,
    streams: StreamMap<String, EventStream>,
    tool_timeout: Duration,
    token_tracker: TokenTracker,
    compaction_config: Option<CompactionConfig>,
    auto_continue: AutoContinue,
    retry_config: RetryConfig,
    active_requests: HashMap<String, ToolCallRequest>,
    queued_user_messages: VecDeque<Vec<llm::ContentBlock>>,
    context_window: Option<u32>,
}

impl Agent {
    pub(crate) fn new(
        config: AgentConfig,
        user_message_rx: mpsc::Receiver<UserMessage>,
        message_tx: mpsc::Sender<AgentMessage>,
    ) -> Self {
        let mut streams: StreamMap<String, EventStream> = StreamMap::new();
        streams.insert(
            USER_STREAM_KEY.to_string(),
            Box::pin(ReceiverStream::new(user_message_rx).map(StreamEvent::UserMessage)),
        );

        let context_limit = config.context_window.or_else(|| config.llm.context_window());

        Self {
            llm: config.llm,
            context: config.context,
            mcp_command_tx: config.mcp_command_tx,
            message_tx,
            streams,
            tool_timeout: config.tool_timeout,
            token_tracker: TokenTracker::new(context_limit),
            compaction_config: config.compaction_config,
            auto_continue: config.auto_continue,
            retry_config: config.retry_config,
            active_requests: HashMap::new(),
            queued_user_messages: VecDeque::new(),
            context_window: config.context_window,
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
        let mut state = IterationState::new();

        while let Some((_, event)) = self.streams.next().await {
            use UserMessage::{Cancel, ClearContext, SetReasoningEffort, SwitchModel, Text, UpdateTools};
            match event {
                StreamEvent::UserMessage(Cancel) => {
                    self.on_user_cancel(&mut state).await;
                }

                StreamEvent::UserMessage(ClearContext) => {
                    self.on_user_clear_context(&mut state).await;
                }

                StreamEvent::UserMessage(Text { content }) => {
                    if self.is_busy() {
                        self.queued_user_messages.push_back(content);
                    } else {
                        state = IterationState::new();
                        self.on_user_text(content);
                    }
                }

                StreamEvent::UserMessage(SwitchModel(new_provider)) => {
                    self.on_switch_model(new_provider).await;
                }

                StreamEvent::UserMessage(UpdateTools(tools)) => {
                    self.context.set_tools(tools);
                }

                StreamEvent::UserMessage(SetReasoningEffort(effort)) => {
                    self.context.set_reasoning_effort(effort);
                }

                StreamEvent::Llm(llm_event) => {
                    self.on_llm_event(llm_event, &mut state).await;
                }

                StreamEvent::ToolExecution(tool_event) => {
                    self.on_tool_execution_event(tool_event, &mut state).await;
                }
            }

            if state.is_complete() {
                let Some(id) = state.current_message_id.take() else {
                    continue;
                };
                let iteration = std::mem::replace(&mut state, IterationState::new());
                self.on_iteration_complete(id, iteration).await;
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
            self.update_context(&message_content, reasoning, completed_tool_calls);

            let _ = self
                .message_tx
                .send(AgentMessage::Text {
                    message_id: id.clone(),
                    chunk: message_content.clone(),
                    is_complete: true,
                    model_name: self.llm.display_name(),
                })
                .await;

            if !reasoning_summary_text.is_empty() {
                let _ = self
                    .message_tx
                    .send(AgentMessage::Thought {
                        message_id: id.clone(),
                        chunk: reasoning_summary_text,
                        is_complete: true,
                        model_name: self.llm.display_name(),
                    })
                    .await;
            }
        }

        let has_queued_text = !self.queued_user_messages.is_empty();
        if has_queued_text {
            let content: Vec<_> = self.queued_user_messages.drain(..).flatten().collect();
            self.context.add_message(ChatMessage::User { content, timestamp: IsoString::now() });
        }

        if has_queued_text || has_tool_calls {
            self.auto_continue.reset();
            self.start_next_turn().await;
        } else if should_auto_continue {
            self.auto_continue.advance();
            tracing::info!(
                "LLM stopped with {:?}, auto-continuing (attempt {}/{})",
                stop_reason,
                self.auto_continue.count(),
                self.auto_continue.max()
            );

            let _ = self
                .message_tx
                .send(AgentMessage::AutoContinue {
                    attempt: self.auto_continue.count(),
                    max_attempts: self.auto_continue.max(),
                })
                .await;

            self.inject_continuation_prompt(&message_content, stop_reason.as_ref());
            self.start_next_turn().await;
        } else {
            tracing::debug!("LLM completed turn with stop reason: {:?}", stop_reason);
            self.auto_continue.reset();
            if let Err(e) = self.message_tx.send(AgentMessage::Done).await {
                tracing::warn!("Failed to send Done message: {:?}", e);
            }
        }
    }

    async fn start_next_turn(&mut self) {
        self.maybe_preflight_compact().await;
        self.start_llm_stream(None);
    }

    async fn on_user_cancel(&mut self, state: &mut IterationState) {
        self.abort_in_flight_work();
        *state = IterationState::new();
        let _ = self.message_tx.send(AgentMessage::Cancelled { message: "Processing cancelled".to_string() }).await;
        let _ = self.message_tx.send(AgentMessage::Done).await;
    }

    async fn on_user_clear_context(&mut self, state: &mut IterationState) {
        self.abort_in_flight_work();
        self.context.clear_conversation();
        self.token_tracker.reset_current_usage();
        self.auto_continue.reset();
        *state = IterationState::new();

        let _ = self.message_tx.send(AgentMessage::ContextCleared).await;
    }

    fn on_user_text(&mut self, content: Vec<llm::ContentBlock>) {
        self.context.add_message(ChatMessage::User { content, timestamp: IsoString::now() });
        self.auto_continue.reset();
        self.start_llm_stream(None);
    }

    async fn on_switch_model(&mut self, new_provider: Box<dyn StreamingModelProvider>) {
        let previous = self.llm.display_name();
        let new_context_limit = self.context_window.or_else(|| new_provider.context_window());
        self.llm = Arc::from(new_provider);
        self.token_tracker.reset_current_usage();
        self.token_tracker.set_context_limit(new_context_limit);
        let new = self.llm.display_name();
        let _ = self.message_tx.send(AgentMessage::ModelSwitched { previous, new }).await;

        let _ = self.message_tx.send(self.context_usage_message()).await;
    }

    fn start_llm_stream(&mut self, delay: Option<Duration>) {
        self.streams.remove(LLM_STREAM_KEY);
        let stream: EventStream = match delay {
            None => Box::pin(self.llm.stream_response(&self.context).map(StreamEvent::Llm)),
            Some(delay) => {
                let llm = Arc::clone(&self.llm);
                let context = self.context.clone();
                Box::pin(async_stream::stream! {
                    sleep(delay).await;
                    let mut inner = llm.stream_response(&context);
                    while let Some(item) = inner.next().await {
                        yield StreamEvent::Llm(item);
                    }
                })
            }
        };
        self.streams.insert(LLM_STREAM_KEY.to_string(), stream);
    }

    async fn on_llm_error(&mut self, error: LlmError, state: &mut IterationState) {
        if !error.is_retryable() || state.retry_attempt >= self.retry_config.max_attempts {
            let _ = self.message_tx.send(AgentMessage::Error { message: error.to_string() }).await;
            return;
        }

        state.retry_attempt += 1;
        let delay = self.retry_config.compute_delay(state.retry_attempt);
        let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);

        tracing::warn!(
            attempt = state.retry_attempt,
            max_attempts = self.retry_config.max_attempts,
            delay_ms,
            error = %error,
            "Retrying LLM request after transient failure"
        );

        let _ = self
            .message_tx
            .send(AgentMessage::Retrying {
                attempt: state.retry_attempt,
                max_attempts: self.retry_config.max_attempts,
                delay_ms,
                error: error.to_string(),
            })
            .await;

        // The previous stream may have emitted partial tool-call deltas
        // before interrupting so we drop them to ensure we rebuild tool state
        self.active_requests.clear();
        self.start_llm_stream(Some(delay));
    }

    fn is_busy(&self) -> bool {
        self.streams.contains_key(LLM_STREAM_KEY) || !self.active_requests.is_empty()
    }

    fn abort_in_flight_work(&mut self) {
        self.streams.remove(LLM_STREAM_KEY);
        for stream_key in self.active_requests.keys().cloned().collect::<Vec<_>>() {
            self.streams.remove(&stream_key);
        }
        self.active_requests.clear();
        self.queued_user_messages.clear();
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
                if let Some(id) = &state.current_message_id {
                    let _ = self
                        .message_tx
                        .send(AgentMessage::Thought {
                            message_id: id.clone(),
                            chunk,
                            is_complete: false,
                            model_name: self.llm.display_name(),
                        })
                        .await;
                }
            }

            EncryptedReasoning { id, content } => {
                if let Some(model) = self.llm.model() {
                    state.encrypted_reasoning = Some(EncryptedReasoningContent { id, model, content });
                }
            }

            ToolRequestStart { id, name } => {
                self.handle_tool_request_start(id, name).await;
            }

            ToolRequestArg { id, chunk } => {
                self.handle_tool_request_arg(id, chunk).await;
            }

            ToolRequestComplete { tool_call } => {
                self.handle_tool_completion(tool_call, state).await;
            }

            Done { stop_reason } => {
                state.llm_done = true;
                state.stop_reason = stop_reason;
            }

            Error { message } => {
                let _ = self.message_tx.send(AgentMessage::Error { message }).await;
            }

            Usage { tokens: sample } => {
                self.handle_llm_usage(sample).await;
            }
        }
    }

    async fn handle_llm_text(&mut self, chunk: String, state: &mut IterationState) {
        state.message_content.push_str(&chunk);

        if let Some(id) = &state.current_message_id {
            let _ = self
                .message_tx
                .send(AgentMessage::Text {
                    message_id: id.clone(),
                    chunk,
                    is_complete: false,
                    model_name: self.llm.display_name(),
                })
                .await;
        }
    }

    async fn handle_tool_request_start(&mut self, id: String, name: String) {
        let request = ToolCallRequest { id: id.clone(), name, arguments: String::new() };
        self.active_requests.insert(id, request.clone());

        let _ = self.message_tx.send(AgentMessage::ToolCall { request, model_name: self.llm.display_name() }).await;
    }

    async fn handle_tool_request_arg(&mut self, id: String, chunk: String) {
        let Some(request) = self.active_requests.get_mut(&id) else {
            return;
        };
        request.arguments.push_str(&chunk);

        let _ = self
            .message_tx
            .send(AgentMessage::ToolCallUpdate { tool_call_id: id, chunk, model_name: self.llm.display_name() })
            .await;
    }

    async fn handle_tool_completion(&mut self, tool_call: ToolCallRequest, state: &mut IterationState) {
        state.pending_tool_ids.insert(tool_call.id.clone());
        debug_assert!(
            self.active_requests.contains_key(&tool_call.id),
            "tool call {} should already be in active_requests from handle_tool_request_start",
            tool_call.id
        );

        let (tx, rx) = mpsc::channel(100);
        let stream = ReceiverStream::new(rx).map(StreamEvent::ToolExecution);
        let stream_key = tool_call.id.clone();
        self.streams.insert(stream_key, Box::pin(stream));

        if let Some(ref mcp_command_tx) = self.mcp_command_tx {
            let mcp_future =
                mcp_command_tx.send(McpCommand::ExecuteTool { request: tool_call, timeout: self.tool_timeout, tx });
            if let Err(e) = mcp_future.await {
                tracing::warn!("Failed to send tool request to MCP task: {:?}", e);
            }
        }
    }

    async fn handle_llm_usage(&mut self, sample: TokenUsage) {
        self.token_tracker.record_usage(sample);
        let ratio_pct = self.token_tracker.usage_ratio().map(|r| r * 100.0);
        let remaining = self.token_tracker.tokens_remaining();
        tracing::debug!(?sample, ?ratio_pct, ?remaining, "Token usage");

        let _ = self.message_tx.send(self.context_usage_message()).await;

        self.maybe_compact_context().await;
    }

    fn context_usage_message(&self) -> AgentMessage {
        let last = self.token_tracker.last_usage();
        AgentMessage::ContextUsageUpdate {
            usage_ratio: self.token_tracker.usage_ratio(),
            context_limit: self.token_tracker.context_limit(),
            input_tokens: last.input_tokens,
            output_tokens: last.output_tokens,
            cache_read_tokens: last.cache_read_tokens,
            cache_creation_tokens: last.cache_creation_tokens,
            reasoning_tokens: last.reasoning_tokens,
            total_input_tokens: self.token_tracker.total_input_tokens(),
            total_output_tokens: self.token_tracker.total_output_tokens(),
            total_cache_read_tokens: self.token_tracker.total_cache_read_tokens(),
            total_cache_creation_tokens: self.token_tracker.total_cache_creation_tokens(),
            total_reasoning_tokens: self.token_tracker.total_reasoning_tokens(),
        }
    }

    /// Pre-flight check: estimate context size and compact proactively if it would
    /// overflow before the LLM even sees it. This catches the case where large tool
    /// results push context past the limit before usage-based compaction can fire.
    async fn maybe_preflight_compact(&mut self) {
        let Some(context_limit) = self.token_tracker.context_limit() else {
            return;
        };
        let Some(config) = self.compaction_config.as_ref() else {
            return;
        };
        let estimated = self.context.estimated_token_count();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let threshold = (f64::from(context_limit) * config.threshold).ceil() as u32;
        if estimated >= threshold {
            tracing::info!(
                "Pre-flight compaction triggered: estimated {estimated} tokens >= {:.1}% of {context_limit} limit",
                config.threshold * 100.0
            );
            if let CompactionOutcome::Failed(e) = self.compact_context().await {
                tracing::warn!("Pre-flight compaction failed: {e}");
            }
        }
    }

    /// Check if compaction is needed and perform it if so.
    async fn maybe_compact_context(&mut self) {
        if !self.compaction_config.as_ref().is_some_and(|config| self.token_tracker.should_compact(config.threshold)) {
            return;
        }

        if let CompactionOutcome::Failed(error_message) = self.compact_context().await {
            tracing::warn!("Context compaction failed: {}", error_message);
        }
    }

    async fn compact_context(&mut self) -> CompactionOutcome {
        let Some(ref _config) = self.compaction_config else {
            tracing::warn!("Context compaction requested but compaction is disabled");
            return CompactionOutcome::SkippedDisabled;
        };

        match self.token_tracker.usage_ratio() {
            Some(usage_ratio) => {
                tracing::info!(
                    "Starting context compaction - {} messages, {:.1}% of context limit",
                    self.context.message_count(),
                    usage_ratio * 100.0
                );
            }
            None => {
                tracing::info!(
                    "Starting context compaction - {} messages (context limit unknown)",
                    self.context.message_count(),
                );
            }
        }

        let _ = self
            .message_tx
            .send(AgentMessage::ContextCompactionStarted { message_count: self.context.message_count() })
            .await;

        let compactor = Compactor::new(self.llm.clone());

        match compactor.compact(&self.context).await {
            Ok(result) => {
                tracing::info!("Context compacted: {} messages removed", result.messages_removed);

                self.context = result.context;
                self.token_tracker.reset_current_usage();

                let _ = self
                    .message_tx
                    .send(AgentMessage::ContextCompactionResult {
                        summary: result.summary,
                        messages_removed: result.messages_removed,
                    })
                    .await;
                CompactionOutcome::Compacted
            }
            Err(e) => CompactionOutcome::Failed(e.to_string()),
        }
    }

    async fn on_tool_execution_event(&mut self, event: ToolExecutionEvent, state: &mut IterationState) {
        match event {
            ToolExecutionEvent::Started { tool_id, tool_name } => {
                tracing::debug!("Tool execution started: {} ({})", tool_name, tool_id);
            }

            ToolExecutionEvent::Progress { tool_id, progress } => {
                tracing::debug!(
                    "Tool progress for {}: {}/{}",
                    tool_id,
                    progress.progress,
                    progress.total.unwrap_or(0.0)
                );

                if let Some(request) = self.active_requests.get(&tool_id) {
                    let _ = self
                        .message_tx
                        .send(AgentMessage::ToolProgress {
                            request: request.clone(),
                            progress: progress.progress,
                            total: progress.total,
                            message: progress.message.clone(),
                        })
                        .await;
                }
            }

            ToolExecutionEvent::Complete { tool_id: _, result, result_meta } => match result {
                Ok(tool_result) => {
                    tracing::debug!("Tool result received: {} -> {}", tool_result.name, tool_result.result.len());

                    if state.pending_tool_ids.remove(&tool_result.id) {
                        self.active_requests.remove(&tool_result.id);
                        state.completed_tool_calls.push(Ok(tool_result.clone()));

                        let msg = AgentMessage::ToolResult {
                            result: tool_result,
                            result_meta,
                            model_name: self.llm.display_name(),
                        };

                        if let Err(e) = self.message_tx.send(msg).await {
                            tracing::warn!("Failed to send ToolCall completion message: {:?}", e);
                        }
                    } else {
                        tracing::debug!("Ignoring stale tool result for id: {}", tool_result.id);
                    }
                }

                Err(tool_error) => {
                    if state.pending_tool_ids.remove(&tool_error.id) {
                        self.active_requests.remove(&tool_error.id);
                        state.completed_tool_calls.push(Err(tool_error.clone()));

                        let _ = self
                            .message_tx
                            .send(AgentMessage::ToolError { error: tool_error, model_name: self.llm.display_name() })
                            .await;
                    }
                }
            },
        }
    }

    fn update_context(
        &mut self,
        message_content: &str,
        reasoning: AssistantReasoning,
        completed_tools: Vec<Result<ToolCallResult, ToolCallError>>,
    ) {
        self.context.push_assistant_turn(message_content, reasoning, completed_tools);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompactionOutcome {
    Compacted,
    SkippedDisabled,
    Failed(String),
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

    fn count(&self) -> u32 {
        self.count
    }

    fn max(&self) -> u32 {
        self.max
    }
}

#[derive(Debug)]
struct IterationState {
    current_message_id: Option<String>,
    message_content: String,
    reasoning_summary_text: String,
    encrypted_reasoning: Option<EncryptedReasoningContent>,
    pending_tool_ids: HashSet<String>,
    completed_tool_calls: Vec<Result<ToolCallResult, ToolCallError>>,
    llm_done: bool,
    stop_reason: Option<StopReason>,
    retry_attempt: u32,
}

impl IterationState {
    fn new() -> Self {
        Self {
            current_message_id: None,
            message_content: String::new(),
            reasoning_summary_text: String::new(),
            encrypted_reasoning: None,
            pending_tool_ids: HashSet::new(),
            completed_tool_calls: Vec::new(),
            llm_done: false,
            stop_reason: None,
            retry_attempt: 0,
        }
    }

    fn on_llm_start(&mut self, message_id: String) {
        self.current_message_id = Some(message_id);
        self.message_content.clear();
        self.reasoning_summary_text.clear();
        self.encrypted_reasoning = None;
        self.stop_reason = None;
    }

    fn is_complete(&self) -> bool {
        self.llm_done && self.pending_tool_ids.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::testing::FakeLlmProvider;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_preflight_compaction_uses_configured_threshold() {
        let llm = Arc::new(
            FakeLlmProvider::with_single_response(vec![
                LlmResponse::start("summary"),
                LlmResponse::text("summary"),
                LlmResponse::done(),
            ])
            .with_context_window(Some(100)),
        );
        let context = Context::new(
            vec![ChatMessage::User {
                content: vec![llm::ContentBlock::text("x".repeat(344))],
                timestamp: IsoString::now(),
            }],
            vec![],
        );
        let (user_tx, user_rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(8);
        drop(user_tx);

        let mut agent = Agent::new(
            AgentConfig {
                llm,
                context,
                mcp_command_tx: None,
                tool_timeout: Duration::from_secs(1),
                compaction_config: Some(CompactionConfig::with_threshold(0.85)),
                auto_continue: AutoContinue::new(0),
                retry_config: RetryConfig::disabled(),
                context_window: None,
            },
            user_rx,
            message_tx,
        );

        agent.maybe_preflight_compact().await;

        assert!(
            matches!(
                agent.context.messages().as_slice(),
                [ChatMessage::Summary { content, .. }] if content == "summary"
            ),
            "expected context to be compacted, got {:?}",
            agent.context.messages()
        );
    }
}
