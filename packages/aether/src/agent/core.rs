use crate::agent::middleware::{AgentEvent, Middleware, MiddlewareAction};
use crate::agent::{AgentMessage, UserMessage};
use crate::context::{CompactionConfig, Compactor, TokenTracker};
use crate::llm::{
    ChatMessage, StreamingModelProvider, ToolCallError, ToolCallRequest, ToolCallResult,
};
use crate::llm::{Context, LlmError, LlmResponse};
use crate::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use crate::types::IsoString;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
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

pub struct Agent<T: StreamingModelProvider> {
    llm: Arc<T>,
    context: Context,
    mcp_command_tx: Option<mpsc::Sender<McpCommand>>,
    agent_message_tx: mpsc::Sender<AgentMessage>,
    streams: StreamMap<String, EventStream>,
    middleware: Middleware,
    tool_timeout: Duration,
    token_tracker: TokenTracker,
    compaction_config: Option<CompactionConfig>,
    /// Flag indicating compaction is needed before the next LLM call
    pending_compaction: bool,
}

impl<T: StreamingModelProvider + 'static> Agent<T> {
    pub fn new(
        llm: Arc<T>,
        context: Context,
        mcp_command_tx: Option<mpsc::Sender<McpCommand>>,
        user_message_rx: mpsc::Receiver<UserMessage>,
        agent_message_tx: mpsc::Sender<AgentMessage>,
        middleware: Middleware,
        tool_timeout: Duration,
        compaction_config: Option<CompactionConfig>,
    ) -> Self {
        let mut streams: StreamMap<String, EventStream> = StreamMap::new();
        streams.insert(
            "user".to_string(),
            Box::pin(ReceiverStream::new(user_message_rx).map(StreamEvent::UserMessage)),
        );

        Self {
            llm,
            context,
            mcp_command_tx,
            agent_message_tx,
            streams,
            middleware,
            tool_timeout,
            token_tracker: TokenTracker::new(200_000), // Default to Claude's 200k context limit
            compaction_config,
            pending_compaction: false,
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
        let model_name = self.llm.display_name();

        while let Some((_, event)) = self.streams.next().await {
            use UserMessage::*;
            match event {
                StreamEvent::UserMessage(Cancel) => {
                    self.on_user_cancel(&mut state).await;
                }

                StreamEvent::UserMessage(Text { content }) => {
                    self.on_user_text(content).await;
                }

                StreamEvent::Llm(llm_event) => {
                    if !state.cancelled {
                        self.on_llm_event(llm_event, &mut state).await;
                    }
                }

                StreamEvent::ToolExecution(tool_event) => {
                    if !state.cancelled {
                        self.on_tool_execution_event(tool_event, &mut state).await;
                    }
                }
            }

            if state.is_complete()
                && let Some(ref id) = state.current_message_id
            {
                self.update_context(&state.message_content, &state.completed_tool_calls);
                let _ = self
                    .agent_message_tx
                    .send(AgentMessage::Text {
                        message_id: id.clone(),
                        chunk: state.message_content.clone(), // Send full message content
                        is_complete: true,
                        model_name: model_name.clone(),
                    })
                    .await;

                let should_continue = state.has_tool_calls();
                state = IterationState::new();

                if should_continue {
                    self.start_llm_stream().await;
                } else if let Err(e) = self.agent_message_tx.send(AgentMessage::Done).await {
                    tracing::warn!("Failed to send Done message: {:?}", e);
                }
            }
        }

        tracing::debug!("Agent task shutting down - input channel closed");
    }

    async fn on_user_cancel(&mut self, state: &mut IterationState) {
        state.cancelled = true;
        self.streams.remove("llm");
        let _ = self
            .agent_message_tx
            .send(AgentMessage::Cancelled {
                message: "Processing cancelled".to_string(),
            })
            .await;
        let _ = self.agent_message_tx.send(AgentMessage::Done).await;
    }

    async fn on_user_text(&mut self, content: String) {
        let action = self
            .middleware
            .emit(AgentEvent::UserMessage {
                content: content.clone(),
            })
            .await;

        if action == MiddlewareAction::Block {
            tracing::debug!("User message blocked by middleware");
            let _ = self
                .agent_message_tx
                .send(AgentMessage::Error {
                    message: "Message blocked by middleware".to_string(),
                })
                .await;
            return;
        }

        self.context.add_message(ChatMessage::User {
            content,
            timestamp: IsoString::now(),
        });

        self.start_llm_stream().await;
    }

    async fn start_llm_stream(&mut self) {
        // Perform compaction if needed before starting a new LLM call
        self.maybe_compact_context().await;

        self.streams.remove("llm");

        let llm_stream = self
            .llm
            .stream_response(&self.context)
            .map(StreamEvent::Llm);

        self.streams.insert("llm".to_string(), Box::pin(llm_stream));
    }

    async fn on_llm_event(
        &mut self,
        result: Result<LlmResponse, LlmError>,
        state: &mut IterationState,
    ) {
        use LlmResponse::*;

        let response = match result {
            Ok(response) => response,
            Err(e) => {
                let _ = self
                    .agent_message_tx
                    .send(AgentMessage::Error {
                        message: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        match response {
            Start { message_id } => {
                self.handle_llm_start(message_id, state);
            }

            Text { chunk } => {
                self.handle_llm_text(chunk, state).await;
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

            Done => {
                state.llm_done = true;
            }

            Error { message } => {
                let _ = self
                    .agent_message_tx
                    .send(AgentMessage::Error { message })
                    .await;
            }

            Usage {
                input_tokens,
                output_tokens,
            } => {
                self.handle_llm_usage(input_tokens, output_tokens);
            }
        }
    }

    fn handle_llm_start(&mut self, message_id: String, state: &mut IterationState) {
        state.current_message_id = Some(message_id);
        state.message_content.clear();
    }

    async fn handle_llm_text(&mut self, chunk: String, state: &mut IterationState) {
        state.message_content.push_str(&chunk);

        if let Some(id) = &state.current_message_id {
            let _ = self
                .agent_message_tx
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
        let _ = self
            .agent_message_tx
            .send(AgentMessage::ToolCall {
                request: ToolCallRequest {
                    id,
                    name,
                    arguments: String::new(),
                },
                model_name: self.llm.display_name(),
            })
            .await;
    }

    async fn handle_tool_request_arg(&mut self, id: String, chunk: String) {
        let _ = self
            .agent_message_tx
            .send(AgentMessage::ToolCall {
                request: ToolCallRequest {
                    id,
                    name: String::new(),
                    arguments: chunk,
                },
                model_name: self.llm.display_name(),
            })
            .await;
    }

    async fn handle_tool_completion(
        &mut self,
        tool_call: ToolCallRequest,
        state: &mut IterationState,
    ) {
        let action = self
            .middleware
            .emit(AgentEvent::ToolCall {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
            })
            .await;

        if action == MiddlewareAction::Block {
            tracing::debug!("Tool call '{}' blocked by middleware", tool_call.name);
            let _ = self
                .agent_message_tx
                .send(AgentMessage::Error {
                    message: format!("Tool '{}' blocked by middleware", tool_call.name),
                })
                .await;
            return;
        }

        state
            .pending_tool_calls
            .insert(tool_call.id.clone(), tool_call.clone());

        let msg_future = self.agent_message_tx.send(AgentMessage::ToolCall {
            request: tool_call.clone(),
            model_name: self.llm.display_name(),
        });

        let (tx, rx) = mpsc::channel(100);
        let stream = ReceiverStream::new(rx).map(StreamEvent::ToolExecution);
        let stream_key = tool_call.id.clone();
        self.streams.insert(stream_key, Box::pin(stream));

        if let Some(ref mcp_command_tx) = self.mcp_command_tx {
            let mcp_future = mcp_command_tx.send(McpCommand::ExecuteTool {
                request: tool_call,
                timeout: self.tool_timeout,
                tx,
            });
            let (_, mcp_result) = tokio::join!(msg_future, mcp_future);
            if let Err(e) = mcp_result {
                tracing::warn!("Failed to send tool request to MCP task: {:?}", e);
            }
        }
    }

    fn handle_llm_usage(&mut self, input_tokens: u32, output_tokens: u32) {
        // Record token usage for context management
        self.token_tracker.record_usage(input_tokens, output_tokens);
        tracing::debug!(
            "Token usage - input: {}, output: {}, ratio: {:.2}%, remaining: {}",
            input_tokens,
            output_tokens,
            self.token_tracker.usage_ratio() * 100.0,
            self.token_tracker.tokens_remaining()
        );

        // Check if compaction is needed
        if let Some(ref config) = self.compaction_config {
            if config.auto_compact
                && self.token_tracker.should_compact(config.threshold)
                && self.context.message_count() >= config.min_messages_for_compaction
            {
                tracing::info!(
                    "Context compaction needed - usage at {:.1}% of limit",
                    self.token_tracker.usage_ratio() * 100.0
                );
                self.pending_compaction = true;
            }
        }
    }

    /// Perform context compaction if pending.
    async fn maybe_compact_context(&mut self) {
        if !self.pending_compaction {
            return;
        }

        self.pending_compaction = false;

        tracing::info!(
            "Starting context compaction - {} messages, {:.1}% of context limit",
            self.context.message_count(),
            self.token_tracker.usage_ratio() * 100.0
        );

        let compactor = Compactor::new(self.llm.clone());

        match compactor.compact(&mut self.context).await {
            Ok(result) => {
                tracing::info!(
                    "Context compacted: {} messages removed",
                    result.messages_removed
                );

                // Emit middleware event
                let _ = self
                    .middleware
                    .emit(AgentEvent::ContextCompacted {
                        summary_length: result.summary.len(),
                        messages_removed: result.messages_removed,
                    })
                    .await;

                // Send message to consumers
                let _ = self
                    .agent_message_tx
                    .send(AgentMessage::ContextCompacted {
                        summary: result.summary,
                        messages_removed: result.messages_removed,
                    })
                    .await;
            }
            Err(e) => {
                tracing::warn!("Context compaction failed: {}", e);
                // Don't block the agent - just log and continue
            }
        }
    }

    async fn on_tool_execution_event(
        &mut self,
        event: ToolExecutionEvent,
        state: &mut IterationState,
    ) {
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

                if let Some(request) = state.pending_tool_calls.get(&tool_id) {
                    let _ = self
                        .agent_message_tx
                        .send(AgentMessage::ToolProgress {
                            request: request.clone(),
                            progress: progress.progress,
                            total: progress.total,
                            message: progress.message.clone(),
                        })
                        .await;
                }
            }

            ToolExecutionEvent::Complete { tool_id: _, result } => match result {
                Ok(tool_result) => {
                    tracing::debug!(
                        "Tool result received: {} -> {}",
                        tool_result.name,
                        tool_result.result.len()
                    );

                    if let Some(_request) = state.pending_tool_calls.remove(&tool_result.id) {
                        state.completed_tool_calls.push(Ok(tool_result.clone()));

                        let msg = AgentMessage::ToolResult {
                            result: tool_result,
                            model_name: self.llm.display_name(),
                        };

                        if let Err(e) = self.agent_message_tx.send(msg).await {
                            tracing::warn!("Failed to send ToolCall completion message: {:?}", e);
                        }
                    } else {
                        tracing::debug!("Ignoring stale tool result for id: {}", tool_result.id);
                    }
                }

                Err(tool_error) => {
                    if let Some(_request) = state.pending_tool_calls.remove(&tool_error.id) {
                        state.completed_tool_calls.push(Err(tool_error.clone()));

                        let _ = self
                            .agent_message_tx
                            .send(AgentMessage::ToolError {
                                error: tool_error,
                                model_name: self.llm.display_name(),
                            })
                            .await;
                    }
                }
            },
        }
    }

    fn update_context(
        &mut self,
        message_content: &str,
        completed_tools: &[Result<ToolCallResult, ToolCallError>],
    ) {
        let tool_requests: Vec<_> = completed_tools
            .iter()
            .map(|result| match result {
                Ok(result) => ToolCallRequest {
                    id: result.id.clone(),
                    name: result.name.clone(),
                    arguments: result.arguments.clone(),
                },
                Err(error) => ToolCallRequest {
                    id: error.id.clone(),
                    name: error.name.clone(),
                    arguments: error.arguments.clone().unwrap_or_default(),
                },
            })
            .collect();

        self.context.add_message(ChatMessage::Assistant {
            content: message_content.to_string(),
            timestamp: IsoString::now(),
            tool_calls: tool_requests,
        });

        for result in completed_tools {
            self.context
                .add_message(ChatMessage::ToolCallResult(result.clone()));
        }
    }
}

#[derive(Debug)]
struct IterationState {
    pub current_message_id: Option<String>,
    pub message_content: String,
    pub pending_tool_calls: HashMap<String, ToolCallRequest>,
    pub completed_tool_calls: Vec<Result<ToolCallResult, ToolCallError>>,
    pub llm_done: bool,
    pub cancelled: bool,
}

impl IterationState {
    pub fn new() -> Self {
        Self {
            current_message_id: None,
            message_content: String::new(),
            pending_tool_calls: HashMap::new(),
            completed_tool_calls: Vec::new(),
            llm_done: false,
            cancelled: false,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.llm_done && self.pending_tool_calls.is_empty()
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.completed_tool_calls.is_empty()
    }
}
