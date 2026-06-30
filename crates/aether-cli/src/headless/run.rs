use aether_core::core::Prompt;
use aether_core::events::{AgentMessage, Command};
use aether_core::mcp::run_mcp_task::McpCommand;
use std::io;
use std::process::ExitCode;
use tokio::sync::mpsc;
use tracing::error;

use super::error::CliError;
use super::{CliEventKind, RunConfig};
use crate::output::OutputFormat;
use crate::runtime::RuntimeBuilder;
use crate::slash_commands::{expand_slash_command, parse_slash_command};

pub async fn run(config: RunConfig) -> Result<ExitCode, CliError> {
    setup_tracing(config.verbose);

    let mut spec = config.spec;
    if let Some(system_prompt) = config.system_prompt {
        spec.prompts.push(Prompt::text(&system_prompt));
    }

    let (agent, _mcp_snapshot) = RuntimeBuilder::from_spec(config.cwd.clone(), spec)
        .mcp_sources(config.mcp_config_sources)
        .oauth_credential_store(config.oauth_credential_store)
        .build_ready(vec![])
        .await?;

    let prompt = expand_prompt(&agent.mcp_tx, config.prompt).await;

    agent
        .agent_tx
        .send(Command::text(&prompt))
        .await
        .map_err(|e| CliError::AgentError(format!("Failed to send prompt: {e}")))?;

    Ok(stream_output(agent.agent_rx, config.output, &config.events).await)
}

async fn expand_prompt(mcp_tx: &mpsc::Sender<McpCommand>, prompt: String) -> String {
    let Some(slash_command) = parse_slash_command(&prompt) else {
        return prompt;
    };

    match expand_slash_command(mcp_tx, slash_command.command_name, slash_command.args_text).await {
        Ok(expanded) => expanded,
        Err(error) => {
            error!("Failed to expand slash command: {error}");
            prompt
        }
    }
}

async fn stream_output(
    mut rx: mpsc::Receiver<AgentMessage>,
    format: OutputFormat,
    events: &[CliEventKind],
) -> ExitCode {
    while let Some(msg) = rx.recv().await {
        if should_emit(&msg, events)
            && let Err(error) = print_message(format, &msg)
        {
            eprintln!("Failed to serialize headless event: {error}");
            return ExitCode::FAILURE;
        }

        if matches!(msg, AgentMessage::Done) {
            break;
        }
    }
    ExitCode::SUCCESS
}

fn print_message(format: OutputFormat, msg: &AgentMessage) -> Result<(), serde_json::Error> {
    match format {
        OutputFormat::Text => {
            if let Some(text) = format_text(msg) {
                if matches!(msg, AgentMessage::Error { .. }) {
                    eprintln!("{text}");
                } else {
                    println!("{text}");
                }
            }
        }
        OutputFormat::Pretty => println!("{}", serde_json::to_string_pretty(msg)?),
        OutputFormat::Json => println!("{}", serde_json::to_string(msg)?),
    }

    Ok(())
}

fn should_emit(msg: &AgentMessage, include: &[CliEventKind]) -> bool {
    let Some(kind) = event_kind(msg) else { return false };
    include.is_empty() || include.contains(&kind)
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
        AgentMessage::Done => Some(CliEventKind::Done),
        AgentMessage::Text { is_complete: false, .. }
        | AgentMessage::Thought { is_complete: false, .. }
        | AgentMessage::ToolCallUpdate { .. } => None,
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

        AgentMessage::ContextUsageUpdate { usage } => Some(format!(
            "Tokens: {} in, {} out (total: {} in, {} out)",
            usage.input_tokens, usage.output_tokens, usage.total_input_tokens, usage.total_output_tokens
        )),

        AgentMessage::ContextCleared => Some("Context cleared".to_string()),

        AgentMessage::Done => Some("Done".to_string()),

        AgentMessage::ToolCallUpdate { .. } | AgentMessage::Text { .. } | AgentMessage::Thought { .. } => None,
    }
}

fn setup_tracing(verbose: bool) {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::EnvFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = if verbose { EnvFilter::new("debug,agent=off") } else { EnvFilter::new("error,agent=off") };
    let layer = fmt::layer().with_writer(io::stderr).with_filter(filter);

    tracing_subscriber::registry().with(layer).init();
}

#[cfg(test)]
mod tests {
    use aether_core::events::ContextUsage;

    use super::*;

    #[test]
    fn format_text_formats_complete_text() {
        assert_eq!(format_text(&AgentMessage::text("id", "hello world", true, "m")), Some("hello world".to_string()));
    }

    #[test]
    fn format_text_skips_incomplete_text() {
        assert_eq!(format_text(&AgentMessage::text("id", "partial", false, "m")), None);
    }

    #[test]
    fn format_text_formats_complete_thought() {
        assert_eq!(
            format_text(&AgentMessage::thought("id", "reasoning here", true, "m")),
            Some("Thought: reasoning here".to_string())
        );
    }

    #[test]
    fn format_text_skips_incomplete_thought() {
        assert_eq!(format_text(&AgentMessage::thought("id", "partial", false, "m")), None);
    }

    #[test]
    fn format_text_formats_tool_call() {
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
    fn format_text_skips_tool_call_updates() {
        let msg = AgentMessage::ToolCallUpdate {
            tool_call_id: "tc1".to_string(),
            chunk: "partial".to_string(),
            model_name: "test".to_string(),
        };
        assert_eq!(format_text(&msg), None);
    }

    #[test]
    fn format_text_formats_tool_result() {
        assert_eq!(format_text(&tool_result_msg()), Some("Tool result [bash]: ok".to_string()));
    }

    #[test]
    fn format_text_formats_tool_error() {
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
    fn format_text_formats_error() {
        let msg = AgentMessage::Error { message: "boom".to_string() };
        assert_eq!(format_text(&msg), Some("Error: boom".to_string()));
    }

    #[test]
    fn format_text_formats_cancelled() {
        let msg = AgentMessage::Cancelled { message: "user stopped".to_string() };
        assert_eq!(format_text(&msg), Some("Cancelled: user stopped".to_string()));
    }

    #[test]
    fn format_text_formats_auto_continue() {
        let msg = AgentMessage::AutoContinue { attempt: 2, max_attempts: 5 };
        assert_eq!(format_text(&msg), Some("Continuing (2/5)...".to_string()));
    }

    #[test]
    fn format_text_formats_model_switched() {
        let msg = AgentMessage::ModelSwitched { previous: "old-model".to_string(), new: "new-model".to_string() };
        assert_eq!(format_text(&msg), Some("Model switched: old-model -> new-model".to_string()));
    }

    #[test]
    fn format_text_renders_done() {
        assert_eq!(format_text(&AgentMessage::Done), Some("Done".to_string()));
    }

    #[test]
    fn format_text_formats_tool_progress_with_total() {
        let msg = tool_progress(50.0, Some(100.0), Some("halfway"));
        assert_eq!(format_text(&msg), Some("Tool progress [bash]: 50/100 - halfway".to_string()));
    }

    #[test]
    fn format_text_formats_tool_progress_without_total() {
        let msg = tool_progress(42.0, None, None);
        assert_eq!(format_text(&msg), Some("Tool progress [bash]: 42".to_string()));
    }

    #[test]
    fn format_text_formats_context_compaction_started() {
        let msg = AgentMessage::ContextCompactionStarted { message_count: 42 };
        assert_eq!(format_text(&msg), Some("Context compaction started (42 messages)".to_string()));
    }

    #[test]
    fn format_text_formats_context_compaction_result() {
        let msg = AgentMessage::ContextCompactionResult { summary: "summary here".to_string(), messages_removed: 10 };
        assert_eq!(format_text(&msg), Some("Context compacted: 10 messages removed. summary here".to_string()));
    }

    #[test]
    fn format_text_formats_context_usage_update() {
        assert_eq!(
            format_text(&usage_update()),
            Some("Tokens: 1500 in, 250 out (total: 5000 in, 800 out)".to_string())
        );
    }

    #[test]
    fn format_text_formats_context_cleared() {
        assert_eq!(format_text(&AgentMessage::ContextCleared), Some("Context cleared".to_string()));
    }

    #[test]
    fn event_kind_none_for_non_output_fragments() {
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
    fn event_kind_done_is_filterable() {
        assert_eq!(event_kind(&AgentMessage::Done), Some(CliEventKind::Done));
    }

    #[test]
    fn should_emit_empty_filter_rejects_non_output_events() {
        assert!(should_emit(&tool_call_msg(), &[]));
        assert!(should_emit(&AgentMessage::Error { message: "e".to_string() }, &[]));
        assert!(should_emit(&AgentMessage::Done, &[]));
        assert!(!should_emit(&AgentMessage::text("id", "x", false, "m"), &[]));
        assert!(!should_emit(
            &AgentMessage::ToolCallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
                model_name: "m".to_string(),
            },
            &[],
        ));
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
    fn should_emit_done_respects_filter() {
        assert!(should_emit(&AgentMessage::Done, &[CliEventKind::Done]));
        assert!(!should_emit(&AgentMessage::Done, &[CliEventKind::ToolCall]));
    }

    #[test]
    fn cli_event_kind_names_match_agent_message_type_tags() {
        use clap::ValueEnum;

        let samples = vec![
            (AgentMessage::text("id", "x", true, "m"), CliEventKind::Text),
            (AgentMessage::thought("id", "x", true, "m"), CliEventKind::Thought),
            (tool_call_msg(), CliEventKind::ToolCall),
            (tool_result_msg(), CliEventKind::ToolResult),
            (
                AgentMessage::ToolError {
                    error: llm::ToolCallError {
                        id: "tc1".to_string(),
                        name: "bash".to_string(),
                        arguments: None,
                        error: "boom".to_string(),
                    },
                    model_name: "test".to_string(),
                },
                CliEventKind::ToolError,
            ),
            (AgentMessage::Error { message: "e".to_string() }, CliEventKind::Error),
            (AgentMessage::Cancelled { message: "c".to_string() }, CliEventKind::Cancelled),
            (AgentMessage::AutoContinue { attempt: 1, max_attempts: 3 }, CliEventKind::AutoContinue),
            (
                AgentMessage::Retrying { attempt: 1, max_attempts: 3, delay_ms: 10, error: "e".to_string() },
                CliEventKind::Retrying,
            ),
            (
                AgentMessage::ModelSwitched { previous: "a".to_string(), new: "b".to_string() },
                CliEventKind::ModelSwitched,
            ),
            (tool_progress(1.0, None, None), CliEventKind::ToolProgress),
            (AgentMessage::ContextCompactionStarted { message_count: 1 }, CliEventKind::ContextCompactionStarted),
            (
                AgentMessage::ContextCompactionResult { summary: "s".to_string(), messages_removed: 1 },
                CliEventKind::ContextCompactionResult,
            ),
            (usage_update(), CliEventKind::ContextUsage),
            (AgentMessage::ContextCleared, CliEventKind::ContextCleared),
            (AgentMessage::Done, CliEventKind::Done),
        ];

        for kind in CliEventKind::value_variants() {
            assert!(samples.iter().any(|(_, k)| k == kind), "samples is missing a case for {kind:?}");
        }

        for (msg, kind) in &samples {
            assert_eq!(event_kind(msg), Some(*kind), "event_kind disagrees for {kind:?}");
            let tag = serde_json::to_value(msg).unwrap();
            let tag = tag["type"].as_str().unwrap().to_string();
            let clap_name = kind.to_possible_value().unwrap();
            assert_eq!(tag, clap_name.get_name(), "`--events` value and serialized `type` tag diverged for {kind:?}");
        }
    }

    #[tokio::test]
    async fn stream_output_done_breaks_loop_under_filter() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentMessage::Done).await.unwrap();
        let filter = vec![CliEventKind::ToolCall];
        let code = stream_output(rx, OutputFormat::Text, &filter).await;
        assert_eq!(code, ExitCode::SUCCESS);
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
            usage: ContextUsage {
                input_tokens: 1500,
                output_tokens: 250,
                total_input_tokens: 5000,
                total_output_tokens: 800,
                ..Default::default()
            },
        }
    }
}
