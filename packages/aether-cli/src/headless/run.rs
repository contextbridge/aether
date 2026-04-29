use aether_core::core::Prompt;
use aether_core::events::{AgentMessage, UserMessage};
use std::io;
use std::process::ExitCode;
use tokio::sync::mpsc;

use super::error::CliError;
use super::{CliEventKind, OutputFormat, RunConfig};
use crate::runtime::RuntimeBuilder;

pub async fn run(config: RunConfig) -> Result<ExitCode, CliError> {
    setup_tracing(config.verbose, &config.output);

    let agent = RuntimeBuilder::from_spec(config.cwd.clone(), config.spec)
        .mcp_sources(config.mcp_config_sources)
        .build(config.system_prompt.as_deref().map(Prompt::text), None)
        .await?;

    agent
        .agent_tx
        .send(UserMessage::text(&config.prompt))
        .await
        .map_err(|e| CliError::AgentError(format!("Failed to send prompt: {e}")))?;

    Ok(stream_output(agent.agent_rx, &config.output, &config.events).await)
}

async fn stream_output(
    mut rx: mpsc::Receiver<AgentMessage>,
    format: &OutputFormat,
    events: &[CliEventKind],
) -> ExitCode {
    while let Some(msg) = rx.recv().await {
        if should_emit(&msg, events) {
            match format {
                OutputFormat::Text => emit_text(&msg),
                OutputFormat::Pretty | OutputFormat::Json => emit_event(&msg),
            }
        }
        if matches!(msg, AgentMessage::Done) {
            break;
        }
    }
    ExitCode::SUCCESS
}

fn should_emit(msg: &AgentMessage, include: &[CliEventKind]) -> bool {
    if include.is_empty() {
        return true;
    }
    event_kind(msg).is_none_or(|ty| include.contains(&ty))
}

fn event_kind(msg: &AgentMessage) -> Option<CliEventKind> {
    match msg {
        AgentMessage::Text { is_complete: true, .. } => Some(CliEventKind::Text),
        AgentMessage::Thought { is_complete: true, .. } => Some(CliEventKind::Thought),
        AgentMessage::ToolCall { .. } => Some(CliEventKind::ToolCall),
        AgentMessage::ToolResult { .. } => Some(CliEventKind::ToolResult),
        AgentMessage::ToolError { .. } => Some(CliEventKind::ToolError),
        AgentMessage::Error { .. } => Some(CliEventKind::Error),
        AgentMessage::Cancelled { .. } => Some(CliEventKind::Cancelled),
        AgentMessage::AutoContinue { .. } => Some(CliEventKind::AutoContinue),
        AgentMessage::Retrying { .. } => Some(CliEventKind::Retrying),
        AgentMessage::ModelSwitched { .. } => Some(CliEventKind::ModelSwitched),
        AgentMessage::ToolProgress { .. } => Some(CliEventKind::ToolProgress),
        AgentMessage::ContextCompactionStarted { .. } => Some(CliEventKind::ContextCompactionStarted),
        AgentMessage::ContextCompactionResult { .. } => Some(CliEventKind::ContextCompactionResult),
        AgentMessage::ContextUsageUpdate { .. } => Some(CliEventKind::ContextUsage),
        AgentMessage::ContextCleared => Some(CliEventKind::ContextCleared),
        AgentMessage::Text { is_complete: false, .. }
        | AgentMessage::Thought { is_complete: false, .. }
        | AgentMessage::ToolCallUpdate { .. }
        | AgentMessage::Done => None,
    }
}

fn format_text(msg: &AgentMessage) -> Option<String> {
    match msg {
        AgentMessage::Text { chunk, is_complete: true, .. } => Some(chunk.clone()),

        AgentMessage::Thought { chunk, is_complete: true, .. } => Some(format!("Thought: {chunk}")),

        AgentMessage::ToolCall { request, .. } => Some(format!("Tool call: {}({})", request.name, request.arguments)),

        AgentMessage::ToolResult { result, .. } => Some(format!("Tool result [{}]: {}", result.name, result.result)),

        AgentMessage::ToolError { error, .. } => Some(format!("Tool error [{}]: {}", error.name, error.error)),

        AgentMessage::Error { message } => Some(format!("Error: {message}")),

        AgentMessage::Cancelled { message } => Some(format!("Cancelled: {message}")),

        AgentMessage::AutoContinue { attempt, max_attempts } => {
            Some(format!("Continuing ({attempt}/{max_attempts})..."))
        }

        AgentMessage::Retrying { attempt, max_attempts, delay_ms, error } => {
            Some(format!("Retrying ({attempt}/{max_attempts}) in {delay_ms}ms: {error}"))
        }

        AgentMessage::ModelSwitched { previous, new } => Some(format!("Model switched: {previous} -> {new}")),

        AgentMessage::ToolProgress { request, progress, total, message } => {
            let bar = match total {
                Some(t) => format!("{progress}/{t}"),
                None => format!("{progress}"),
            };
            let suffix = message.as_deref().map(|m| format!(" - {m}")).unwrap_or_default();
            Some(format!("Tool progress [{}]: {bar}{suffix}", request.name))
        }

        AgentMessage::ContextCompactionStarted { message_count } => {
            Some(format!("Context compaction started ({message_count} messages)"))
        }

        AgentMessage::ContextCompactionResult { summary, messages_removed } => {
            Some(format!("Context compacted: {messages_removed} messages removed. {summary}"))
        }

        AgentMessage::ContextUsageUpdate {
            input_tokens,
            output_tokens,
            total_input_tokens,
            total_output_tokens,
            ..
        } => Some(format!(
            "Tokens: {input_tokens} in, {output_tokens} out (total: {total_input_tokens} in, {total_output_tokens} out)"
        )),

        AgentMessage::ContextCleared => Some("Context cleared".to_string()),

        AgentMessage::ToolCallUpdate { .. }
        | AgentMessage::Text { .. }
        | AgentMessage::Thought { .. }
        | AgentMessage::Done => None,
    }
}

fn emit_text(msg: &AgentMessage) {
    if let Some(text) = format_text(msg) {
        if matches!(msg, AgentMessage::Error { .. }) {
            eprintln!("{text}");
        } else {
            println!("{text}");
        }
    }
}

#[allow(clippy::too_many_lines)]
fn emit_event(msg: &AgentMessage) {
    let kind = event_kind(msg).map_or("", CliEventKind::as_str);
    match msg {
        AgentMessage::Text { chunk, is_complete: true, .. } => {
            tracing::info!(target: "agent", kind, "{chunk}");
        }

        AgentMessage::Thought { chunk, is_complete: true, .. } => {
            tracing::info!(target: "agent", kind, thought = %chunk);
        }

        AgentMessage::ToolCall { request, .. } => {
            tracing::info!(
                target: "agent",
                kind,
                tool = %request.name,
                arguments = %request.arguments,
            );
        }

        AgentMessage::ToolResult { result, .. } => {
            tracing::info!(
                target: "agent",
                kind,
                tool = %result.name,
                result = %result.result,
            );
        }

        AgentMessage::ToolError { error, .. } => {
            tracing::warn!(
                target: "agent",
                kind,
                tool = %error.name,
                error = %error.error,
            );
        }

        AgentMessage::Error { message } => {
            tracing::error!(target: "agent", kind, "{message}");
        }

        AgentMessage::Cancelled { message } => {
            tracing::info!(target: "agent", kind, cancelled = %message);
        }

        AgentMessage::AutoContinue { attempt, max_attempts } => {
            tracing::info!(
                target: "agent",
                kind,
                attempt,
                max_attempts,
                "Continuing ({attempt}/{max_attempts})..."
            );
        }

        AgentMessage::Retrying { attempt, max_attempts, delay_ms, error } => {
            tracing::info!(
                target: "agent",
                kind,
                attempt,
                max_attempts,
                delay_ms,
                error = %error,
                "Retrying ({attempt}/{max_attempts}) in {delay_ms}ms: {error}"
            );
        }

        AgentMessage::ModelSwitched { previous, new } => {
            tracing::info!(
                target: "agent",
                kind,
                previous = %previous,
                new = %new,
                "Model switched: {previous} -> {new}"
            );
        }

        AgentMessage::ToolProgress { request, progress, total, message } => {
            tracing::info!(
                target: "agent",
                kind,
                tool = %request.name,
                progress,
                total = ?total,
                message = ?message,
            );
        }

        AgentMessage::ContextCompactionStarted { message_count } => {
            tracing::info!(
                target: "agent",
                kind,
                message_count,
                "context compaction started"
            );
        }

        AgentMessage::ContextCompactionResult { summary, messages_removed } => {
            tracing::info!(
                target: "agent",
                kind,
                messages_removed,
                summary = %summary,
                "context compaction result"
            );
        }

        AgentMessage::ContextUsageUpdate {
            usage_ratio,
            context_limit,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            reasoning_tokens,
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            total_cache_creation_tokens,
            total_reasoning_tokens,
        } => {
            tracing::info!(
                target: "agent",
                kind,
                usage_ratio = ?usage_ratio,
                context_limit = ?context_limit,
                input_tokens,
                output_tokens,
                cache_read_tokens = cache_read_tokens.unwrap_or(0),
                cache_creation_tokens = cache_creation_tokens.unwrap_or(0),
                reasoning_tokens = reasoning_tokens.unwrap_or(0),
                total_input_tokens,
                total_output_tokens,
                total_cache_read_tokens,
                total_cache_creation_tokens,
                total_reasoning_tokens,
                "context usage"
            );
        }

        AgentMessage::ContextCleared => {
            tracing::info!(target: "agent", kind, "context cleared");
        }

        AgentMessage::ToolCallUpdate { .. }
        | AgentMessage::Text { .. }
        | AgentMessage::Thought { .. }
        | AgentMessage::Done => {}
    }
}

fn setup_tracing(verbose: bool, format: &OutputFormat) {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::{self, EnvFilter};
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let diag_filter = if verbose { EnvFilter::new("debug,agent=off") } else { EnvFilter::new("error,agent=off") };

    let diag_layer = fmt::layer().with_writer(io::stderr).with_filter(diag_filter);

    let agent_filter = filter::filter_fn(|meta| meta.target().starts_with("agent"));

    match format {
        OutputFormat::Text => {
            if verbose {
                tracing_subscriber::registry().with(diag_layer).init();
            } else {
                // No tracing output — text mode writes directly to stdout/stderr.
                tracing_subscriber::registry().init();
            }
        }
        OutputFormat::Pretty => {
            let agent_layer = fmt::layer().with_writer(io::stdout).pretty().with_filter(agent_filter);
            tracing_subscriber::registry().with(diag_layer).with(agent_layer).init();
        }
        OutputFormat::Json => {
            let agent_layer = fmt::layer().with_writer(io::stdout).json().with_filter(agent_filter);
            tracing_subscriber::registry().with(diag_layer).with(agent_layer).init();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::Layer;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn emit_event_emits_complete_text() {
        let output = with_test_subscriber(|| {
            emit_event(&AgentMessage::text("id", "hello", true, "model"));
        });
        assert!(output.contains("hello"), "expected 'hello' in: {output}");
    }

    #[test]
    fn emit_event_skips_incomplete_text() {
        let output = with_test_subscriber(|| {
            emit_event(&AgentMessage::text("id", "hello", false, "model"));
        });
        assert!(output.is_empty(), "expected empty output, got: {output}");
    }

    #[test]
    fn emit_event_emits_complete_thought() {
        let output = with_test_subscriber(|| {
            emit_event(&AgentMessage::thought("id", "deep thinking", true, "model"));
        });
        assert!(output.contains("deep thinking"), "expected 'deep thinking' in: {output}");
    }

    #[test]
    fn emit_event_skips_incomplete_thought() {
        let output = with_test_subscriber(|| {
            emit_event(&AgentMessage::thought("id", "partial", false, "model"));
        });
        assert!(output.is_empty(), "expected empty output, got: {output}");
    }

    #[test]
    fn emit_event_emits_tool_call() {
        let msg = AgentMessage::ToolCall {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            model_name: "test".to_string(),
        };
        let output = with_test_subscriber(|| {
            emit_event(&msg);
        });
        assert!(output.contains("bash"), "expected 'bash' in: {output}");
    }

    #[test]
    fn emit_event_skips_tool_call_updates() {
        let msg = AgentMessage::ToolCallUpdate {
            tool_call_id: "tc1".to_string(),
            chunk: "{\"partial".to_string(),
            model_name: "test".to_string(),
        };
        let output = with_test_subscriber(|| {
            emit_event(&msg);
        });
        assert!(output.is_empty(), "expected empty output, got: {output}");
    }

    #[test]
    fn emit_event_emits_tool_result() {
        let msg = AgentMessage::ToolResult {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        };
        let output = with_test_subscriber(|| {
            emit_event(&msg);
        });
        assert!(output.contains("bash"), "expected 'bash' in: {output}");
        assert!(output.contains("ok"), "expected 'ok' in: {output}");
    }

    #[test]
    fn emit_event_emits_error() {
        let msg = AgentMessage::Error { message: "something broke".to_string() };
        let output = with_test_subscriber(|| {
            emit_event(&msg);
        });
        assert!(output.contains("something broke"), "expected 'something broke' in: {output}");
    }

    #[test]
    fn emit_event_skips_done() {
        let output = with_test_subscriber(|| {
            emit_event(&AgentMessage::Done);
        });
        assert!(output.is_empty(), "expected empty output, got: {output}");
    }

    // --- emit_text tests (Text mode) ---

    #[test]
    fn emit_text_formats_complete_text() {
        assert_eq!(format_text(&AgentMessage::text("id", "hello world", true, "m")), Some("hello world".to_string()));
    }

    #[test]
    fn emit_text_skips_incomplete_text() {
        assert_eq!(format_text(&AgentMessage::text("id", "partial", false, "m")), None);
    }

    #[test]
    fn emit_text_formats_complete_thought() {
        assert_eq!(
            format_text(&AgentMessage::thought("id", "reasoning here", true, "m")),
            Some("Thought: reasoning here".to_string())
        );
    }

    #[test]
    fn emit_text_skips_incomplete_thought() {
        assert_eq!(format_text(&AgentMessage::thought("id", "partial", false, "m")), None);
    }

    #[test]
    fn emit_text_formats_tool_call() {
        let msg = AgentMessage::ToolCall {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"cmd":"ls"}"#.to_string(),
            },
            model_name: "test".to_string(),
        };
        assert_eq!(format_text(&msg), Some(r#"Tool call: bash({"cmd":"ls"})"#.to_string()));
    }

    #[test]
    fn emit_text_skips_tool_call_updates() {
        let msg = AgentMessage::ToolCallUpdate {
            tool_call_id: "tc1".to_string(),
            chunk: "partial".to_string(),
            model_name: "test".to_string(),
        };
        assert_eq!(format_text(&msg), None);
    }

    #[test]
    fn emit_text_formats_tool_result() {
        let msg = AgentMessage::ToolResult {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "output".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        };
        assert_eq!(format_text(&msg), Some("Tool result [bash]: output".to_string()));
    }

    #[test]
    fn emit_text_formats_tool_error() {
        let msg = AgentMessage::ToolError {
            error: llm::ToolCallError {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: None,
                error: "not found".to_string(),
            },
            model_name: "test".to_string(),
        };
        assert_eq!(format_text(&msg), Some("Tool error [bash]: not found".to_string()));
    }

    #[test]
    fn emit_text_formats_error() {
        let msg = AgentMessage::Error { message: "boom".to_string() };
        assert_eq!(format_text(&msg), Some("Error: boom".to_string()));
    }

    #[test]
    fn emit_text_formats_cancelled() {
        let msg = AgentMessage::Cancelled { message: "user stopped".to_string() };
        assert_eq!(format_text(&msg), Some("Cancelled: user stopped".to_string()));
    }

    #[test]
    fn emit_text_formats_auto_continue() {
        let msg = AgentMessage::AutoContinue { attempt: 2, max_attempts: 5 };
        assert_eq!(format_text(&msg), Some("Continuing (2/5)...".to_string()));
    }

    #[test]
    fn emit_text_formats_model_switched() {
        let msg = AgentMessage::ModelSwitched { previous: "old-model".to_string(), new: "new-model".to_string() };
        assert_eq!(format_text(&msg), Some("Model switched: old-model -> new-model".to_string()));
    }

    #[test]
    fn emit_text_skips_done() {
        assert_eq!(format_text(&AgentMessage::Done), None);
    }

    fn tool_progress(progress: f64, total: Option<f64>, message: Option<&str>) -> AgentMessage {
        AgentMessage::ToolProgress {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            progress,
            total,
            message: message.map(str::to_string),
        }
    }

    fn usage_update() -> AgentMessage {
        AgentMessage::ContextUsageUpdate {
            usage_ratio: Some(0.25),
            context_limit: Some(200_000),
            input_tokens: 1500,
            output_tokens: 250,
            cache_read_tokens: Some(400),
            cache_creation_tokens: Some(100),
            reasoning_tokens: Some(50),
            total_input_tokens: 5000,
            total_output_tokens: 800,
            total_cache_read_tokens: 1200,
            total_cache_creation_tokens: 300,
            total_reasoning_tokens: 150,
        }
    }

    #[test]
    fn emit_text_formats_tool_progress_with_total() {
        let msg = tool_progress(50.0, Some(100.0), Some("halfway"));
        assert_eq!(format_text(&msg), Some("Tool progress [bash]: 50/100 - halfway".to_string()));
    }

    #[test]
    fn emit_text_formats_tool_progress_without_total() {
        let msg = tool_progress(42.0, None, None);
        assert_eq!(format_text(&msg), Some("Tool progress [bash]: 42".to_string()));
    }

    #[test]
    fn emit_text_formats_context_compaction_started() {
        let msg = AgentMessage::ContextCompactionStarted { message_count: 42 };
        assert_eq!(format_text(&msg), Some("Context compaction started (42 messages)".to_string()));
    }

    #[test]
    fn emit_text_formats_context_compaction_result() {
        let msg = AgentMessage::ContextCompactionResult { summary: "summary here".to_string(), messages_removed: 10 };
        assert_eq!(format_text(&msg), Some("Context compacted: 10 messages removed. summary here".to_string()));
    }

    #[test]
    fn emit_text_formats_context_usage_update() {
        assert_eq!(
            format_text(&usage_update()),
            Some("Tokens: 1500 in, 250 out (total: 5000 in, 800 out)".to_string())
        );
    }

    #[test]
    fn emit_text_formats_context_cleared() {
        assert_eq!(format_text(&AgentMessage::ContextCleared), Some("Context cleared".to_string()));
    }

    #[test]
    fn emit_event_emits_tool_progress() {
        let output = with_test_subscriber(|| emit_event(&tool_progress(3.0, Some(10.0), Some("step"))));
        assert!(output.contains("tool_progress"), "missing type: {output}");
        assert!(output.contains("bash"), "missing tool name: {output}");
        assert!(output.contains('3'), "missing progress: {output}");
    }

    #[test]
    fn emit_event_emits_context_compaction_started() {
        let msg = AgentMessage::ContextCompactionStarted { message_count: 7 };
        let output = with_test_subscriber(|| emit_event(&msg));
        assert!(output.contains("context_compaction_started"), "missing type: {output}");
        assert!(output.contains('7'), "missing message_count: {output}");
    }

    #[test]
    fn emit_event_emits_context_compaction_result() {
        let msg = AgentMessage::ContextCompactionResult { summary: "done".to_string(), messages_removed: 5 };
        let output = with_test_subscriber(|| emit_event(&msg));
        assert!(output.contains("context_compaction_result"), "missing type: {output}");
        assert!(output.contains("done"), "missing summary: {output}");
    }

    #[test]
    fn emit_event_emits_context_usage_update() {
        let output = with_test_subscriber(|| emit_event(&usage_update()));
        assert!(output.contains("context_usage"), "missing type: {output}");
        assert!(output.contains("1500"), "missing input_tokens: {output}");
        assert!(output.contains("5000"), "missing total_input_tokens: {output}");
    }

    #[test]
    fn emit_event_emits_context_cleared() {
        let output = with_test_subscriber(|| emit_event(&AgentMessage::ContextCleared));
        assert!(output.contains("context_cleared"), "missing type: {output}");
    }

    #[test]
    fn emit_event_includes_type_for_tool_call() {
        let msg = AgentMessage::ToolCall {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            model_name: "test".to_string(),
        };
        let output = with_test_subscriber(|| emit_event(&msg));
        assert!(output.contains("tool_call"), "missing type: {output}");
    }

    fn tool_call_msg() -> AgentMessage {
        AgentMessage::ToolCall {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            model_name: "test".to_string(),
        }
    }

    fn tool_result_msg() -> AgentMessage {
        AgentMessage::ToolResult {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        }
    }

    #[test]
    fn event_kind_none_for_non_filterable_variants() {
        assert_eq!(event_kind(&AgentMessage::Done), None);
        assert_eq!(event_kind(&AgentMessage::text("id", "x", false, "m")), None);
        assert_eq!(event_kind(&AgentMessage::thought("id", "x", false, "m")), None);
        assert_eq!(
            event_kind(&AgentMessage::ToolCallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
                model_name: "m".to_string(),
            }),
            None,
        );
    }

    #[test]
    fn should_emit_empty_filter_allows_everything() {
        assert!(should_emit(&tool_call_msg(), &[]));
        assert!(should_emit(&AgentMessage::Error { message: "e".to_string() }, &[]));
        assert!(should_emit(&AgentMessage::Done, &[]));
    }

    #[test]
    fn should_emit_single_type_whitelist() {
        let filter = &[CliEventKind::ToolCall];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(!should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentMessage::Error { message: "e".to_string() }, filter));
    }

    #[test]
    fn should_emit_multi_type_whitelist() {
        let filter = &[CliEventKind::ToolCall, CliEventKind::ToolResult];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentMessage::Error { message: "e".to_string() }, filter));
    }

    #[test]
    fn should_emit_none_typed_variants_pass_through_even_with_filter() {
        let filter = &[CliEventKind::ToolCall];
        assert!(should_emit(&AgentMessage::Done, filter));
        assert!(should_emit(&AgentMessage::text("id", "x", false, "m"), filter));
        assert!(should_emit(
            &AgentMessage::ToolCallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
                model_name: "m".to_string(),
            },
            filter,
        ));
    }

    #[tokio::test]
    async fn stream_output_filter_only_emits_whitelisted_types() {
        let (tx, rx) = mpsc::channel(16);
        tx.send(tool_call_msg()).await.unwrap();
        tx.send(tool_result_msg()).await.unwrap();
        tx.send(AgentMessage::Error { message: "boom".to_string() }).await.unwrap();
        tx.send(AgentMessage::Done).await.unwrap();
        drop(tx);

        let filter = vec![CliEventKind::ToolCall];
        let (_guard, buf) = test_subscriber_guard();

        let code = stream_output(rx, &OutputFormat::Pretty, &filter).await;
        assert_eq!(code, ExitCode::SUCCESS);

        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("tool_call"), "tool_call missing: {output}");
        assert!(!output.contains("tool_result"), "tool_result leaked past filter: {output}");
        assert!(!output.contains("boom"), "error leaked past filter: {output}");
    }

    #[tokio::test]
    async fn stream_output_done_breaks_loop_under_filter() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentMessage::Done).await.unwrap();
        let filter = vec![CliEventKind::ToolCall];
        let code = stream_output(rx, &OutputFormat::Text, &filter).await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    fn test_subscriber_guard() -> (tracing::subscriber::DefaultGuard, Arc<Mutex<Vec<u8>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = Arc::clone(&buf);

        let writer = move || -> TestWriter { TestWriter { buf: Arc::clone(&buf_clone) } };

        let layer = fmt::layer()
            .with_writer(writer)
            .with_ansi(false)
            .with_level(false)
            .with_target(false)
            .with_timer(fmt::time::uptime())
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| meta.target().starts_with("agent")));

        let subscriber = tracing_subscriber::registry().with(layer);
        let guard = tracing::subscriber::set_default(subscriber);
        (guard, buf)
    }

    fn with_test_subscriber<F: FnOnce()>(f: F) -> String {
        let (_guard, buf) = test_subscriber_guard();
        f();
        let bytes = buf.lock().unwrap();
        String::from_utf8(bytes.clone()).unwrap()
    }

    #[derive(Clone)]
    struct TestWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl io::Write for TestWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
