use crate::telemetry::build_telemetry_runtime;
use aether_core::core::Prompt;
use aether_core::events::{
    AgentEvent, Command, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome,
};
use aether_telemetry::TelemetryRuntime;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::error::CliError;
use super::{CliEventKind, RunConfig};
use crate::output::OutputFormat;
use crate::runtime::RuntimeBuilder;
use crate::slash_commands::try_expand_slash_command;

pub async fn run(config: RunConfig) -> Result<ExitCode, CliError> {
    setup_tracing(config.verbose);

    let telemetry = build_telemetry_runtime(&config.telemetry)
        .map_err(|error| CliError::AgentError(format!("failed to initialize telemetry: {error}")))?;

    let result = run_agent(config, telemetry.clone()).await;

    if let Some(telemetry) = telemetry {
        telemetry.shutdown_or_log();
    }
    result
}

async fn run_agent(config: RunConfig, telemetry: Option<Arc<TelemetryRuntime>>) -> Result<ExitCode, CliError> {
    let mut spec = config.spec;
    if let Some(system_prompt) = config.system_prompt {
        spec.prompts.push(Prompt::text(&system_prompt));
    }

    let (agent, _mcp_snapshot) = RuntimeBuilder::from_spec(config.cwd.clone(), spec)
        .mcp_sources(config.mcp_config_sources)
        .oauth_credential_store(config.oauth_credential_store)
        .telemetry_runtime(telemetry)
        .build_ready(vec![])
        .await?;

    let prompt = try_expand_slash_command(&agent.mcp_tx, &config.prompt).await.unwrap_or(config.prompt);

    agent
        .agent_tx
        .send(Command::text(&prompt))
        .await
        .map_err(|e| CliError::AgentError(format!("Failed to send prompt: {e}")))?;

    let exit_code = stream_output(agent.agent_rx, config.output, &config.events).await;

    // Closing the command channel lets the agent task drain; awaiting it
    // guarantees telemetry observers finished their spans before shutdown
    // flushes the exporters.
    drop(agent.agent_tx);
    agent.agent_handle.await_completion().await;

    Ok(exit_code)
}

async fn stream_output(mut rx: mpsc::Receiver<AgentEvent>, format: OutputFormat, events: &[CliEventKind]) -> ExitCode {
    while let Some(msg) = rx.recv().await {
        if should_emit(&msg, events)
            && let Err(error) = print_message(format, &msg)
        {
            eprintln!("Failed to serialize headless event: {error}");
            return ExitCode::FAILURE;
        }

        if let AgentEvent::Turn(TurnEvent::Ended { outcome }) = msg {
            return match outcome {
                TurnOutcome::Failed { .. } => ExitCode::FAILURE,
                TurnOutcome::Completed | TurnOutcome::Cancelled => ExitCode::SUCCESS,
            };
        }
    }
    ExitCode::SUCCESS
}

fn print_message(format: OutputFormat, msg: &AgentEvent) -> Result<(), serde_json::Error> {
    match format {
        OutputFormat::Text => {
            if let Some(text) = format_text(msg) {
                if matches!(msg, AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } })) {
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

fn should_emit(msg: &AgentEvent, include: &[CliEventKind]) -> bool {
    let Some(kind) = event_kind(msg) else { return false };
    include.is_empty() || include.contains(&kind)
}

fn event_kind(msg: &AgentEvent) -> Option<CliEventKind> {
    match msg {
        AgentEvent::Message(MessageEvent::Text { is_complete: true, .. }) => Some(CliEventKind::Text),
        AgentEvent::Message(MessageEvent::Thought { is_complete: true, .. }) => Some(CliEventKind::Thought),
        AgentEvent::Tool(ToolEvent::Call { .. }) => Some(CliEventKind::ToolCall),
        AgentEvent::Tool(ToolEvent::Result { .. }) => Some(CliEventKind::ToolResult),
        AgentEvent::Tool(ToolEvent::Error { .. }) => Some(CliEventKind::ToolError),
        AgentEvent::Turn(TurnEvent::AutoContinue { .. }) => Some(CliEventKind::AutoContinue),
        AgentEvent::Model(ModelEvent::Switched { .. }) => Some(CliEventKind::ModelSwitched),
        AgentEvent::Tool(ToolEvent::Progress { .. }) => Some(CliEventKind::ToolProgress),
        AgentEvent::Context(ContextEvent::CompactionStarted { .. }) => Some(CliEventKind::ContextCompactionStarted),
        AgentEvent::Context(ContextEvent::CompactionResult { .. }) => Some(CliEventKind::ContextCompactionResult),
        AgentEvent::Context(ContextEvent::UsageUpdated { .. }) => Some(CliEventKind::ContextUsage),
        AgentEvent::Context(ContextEvent::Cleared) => Some(CliEventKind::ContextCleared),
        AgentEvent::Turn(TurnEvent::Started { .. }) => Some(CliEventKind::TurnStarted),
        AgentEvent::Turn(TurnEvent::Ended { .. }) => Some(CliEventKind::TurnEnded),
        AgentEvent::Turn(TurnEvent::LlmCallStarted { .. }) => Some(CliEventKind::LlmCallStarted),
        AgentEvent::Turn(TurnEvent::LlmCallEnded { .. }) => Some(CliEventKind::LlmCallEnded),
        AgentEvent::Tool(ToolEvent::ExecutionStarted { .. }) => Some(CliEventKind::ToolExecutionStarted),
        AgentEvent::Tool(ToolEvent::DefinitionsUpdated { .. }) => Some(CliEventKind::ToolDefinitionsUpdated),
        AgentEvent::Message(
            MessageEvent::Text { is_complete: false, .. } | MessageEvent::Thought { is_complete: false, .. },
        )
        | AgentEvent::Tool(ToolEvent::CallUpdate { .. }) => None,
    }
}

fn format_text(msg: &AgentEvent) -> Option<String> {
    match msg {
        AgentEvent::Message(MessageEvent::Text { chunk, is_complete: true, .. }) => Some(chunk.clone()),

        AgentEvent::Message(MessageEvent::Thought { chunk, is_complete: true, .. }) => {
            Some(format!("Thought: {chunk}"))
        }

        AgentEvent::Tool(ToolEvent::Call { request, .. }) => {
            Some(format!("Tool call: {}({})", request.name, request.arguments))
        }

        AgentEvent::Tool(ToolEvent::Result { result, .. }) => {
            Some(format!("Tool result [{}]: {}", result.name, result.result))
        }

        AgentEvent::Tool(ToolEvent::Error { error, .. }) => {
            Some(format!("Tool error [{}]: {}", error.name, error.error))
        }

        AgentEvent::Turn(TurnEvent::Ended { outcome }) => Some(match outcome {
            TurnOutcome::Completed => "Done".to_string(),
            TurnOutcome::Cancelled => "Cancelled".to_string(),
            TurnOutcome::Failed { error } => format!("Error: {error}"),
        }),

        AgentEvent::Turn(TurnEvent::AutoContinue { attempt, max_attempts }) => {
            Some(format!("Continuing ({attempt}/{max_attempts})..."))
        }

        AgentEvent::Turn(event @ TurnEvent::LlmCallStarted { .. }) => event
            .retry_info()
            .map(|retry| format!("Retrying ({}/{}) in {}ms", retry.attempt, retry.max_attempts, retry.delay_ms)),

        AgentEvent::Model(ModelEvent::Switched { previous, new }) => {
            Some(format!("Model switched: {previous} -> {new}"))
        }

        AgentEvent::Tool(ToolEvent::Progress { request, progress, total, message }) => {
            let bar = match total {
                Some(t) => format!("{progress}/{t}"),
                None => format!("{progress}"),
            };
            let suffix = message.as_deref().map(|m| format!(" - {m}")).unwrap_or_default();
            Some(format!("Tool progress [{}]: {bar}{suffix}", request.name))
        }

        AgentEvent::Context(ContextEvent::CompactionStarted { message_count }) => {
            Some(format!("Context compaction started ({message_count} messages)"))
        }

        AgentEvent::Context(ContextEvent::CompactionResult { summary, messages_removed }) => {
            Some(format!("Context compacted: {messages_removed} messages removed. {summary}"))
        }

        AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => Some(format!(
            "Tokens: {} in, {} out (total: {} in, {} out)",
            usage.input_tokens, usage.output_tokens, usage.total_input_tokens, usage.total_output_tokens
        )),

        AgentEvent::Context(ContextEvent::Cleared) => Some("Context cleared".to_string()),

        AgentEvent::Turn(TurnEvent::Started { .. } | TurnEvent::LlmCallEnded { .. })
        | AgentEvent::Tool(
            ToolEvent::ExecutionStarted { .. } | ToolEvent::DefinitionsUpdated { .. } | ToolEvent::CallUpdate { .. },
        )
        | AgentEvent::Message(MessageEvent::Text { .. } | MessageEvent::Thought { .. }) => None,
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
        assert_eq!(format_text(&AgentEvent::text("id", "hello world", true, "m")), Some("hello world".to_string()));
    }

    #[test]
    fn format_text_skips_incomplete_text() {
        assert_eq!(format_text(&AgentEvent::text("id", "partial", false, "m")), None);
    }

    #[test]
    fn format_text_formats_complete_thought() {
        assert_eq!(
            format_text(&AgentEvent::thought("id", "reasoning here", true, "m")),
            Some("Thought: reasoning here".to_string())
        );
    }

    #[test]
    fn format_text_skips_incomplete_thought() {
        assert_eq!(format_text(&AgentEvent::thought("id", "partial", false, "m")), None);
    }

    #[test]
    fn format_text_formats_tool_call() {
        let msg = AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"cmd":"ls"}"#.to_string(),
            },
            model_name: "test".to_string(),
        });
        assert_eq!(format_text(&msg), Some(r#"Tool call: bash({"cmd":"ls"})"#.to_string()));
    }

    #[test]
    fn format_text_skips_tool_call_updates() {
        let msg = AgentEvent::Tool(ToolEvent::CallUpdate {
            tool_call_id: "tc1".to_string(),
            chunk: "partial".to_string(),
            model_name: "test".to_string(),
        });
        assert_eq!(format_text(&msg), None);
    }

    #[test]
    fn format_text_formats_tool_result() {
        assert_eq!(format_text(&tool_result_msg()), Some("Tool result [bash]: ok".to_string()));
    }

    #[test]
    fn format_text_formats_tool_error() {
        let msg = AgentEvent::Tool(ToolEvent::Error {
            error: llm::ToolCallError {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: None,
                error: "not found".to_string(),
            },
            model_name: "test".to_string(),
        });
        assert_eq!(format_text(&msg), Some("Tool error [bash]: not found".to_string()));
    }

    #[test]
    fn format_text_formats_failed_turn() {
        let msg = AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "boom".to_string() } });
        assert_eq!(format_text(&msg), Some("Error: boom".to_string()));
    }

    #[test]
    fn format_text_formats_cancelled_turn() {
        let msg = AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Cancelled });
        assert_eq!(format_text(&msg), Some("Cancelled".to_string()));
    }

    #[test]
    fn format_text_formats_retrying_call_start() {
        let msg = llm_call_started(1, Some(10));
        assert_eq!(format_text(&msg), Some("Retrying (1/3) in 10ms".to_string()));
    }

    #[test]
    fn format_text_skips_first_call_start() {
        assert_eq!(format_text(&llm_call_started(0, None)), None);
    }

    #[test]
    fn format_text_formats_auto_continue() {
        let msg = AgentEvent::Turn(TurnEvent::AutoContinue { attempt: 2, max_attempts: 5 });
        assert_eq!(format_text(&msg), Some("Continuing (2/5)...".to_string()));
    }

    #[test]
    fn format_text_formats_model_switched() {
        let msg =
            AgentEvent::Model(ModelEvent::Switched { previous: "old-model".to_string(), new: "new-model".to_string() });
        assert_eq!(format_text(&msg), Some("Model switched: old-model -> new-model".to_string()));
    }

    #[test]
    fn format_text_renders_completed_turn() {
        assert_eq!(
            format_text(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed })),
            Some("Done".to_string())
        );
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
        let msg = AgentEvent::Context(ContextEvent::CompactionStarted { message_count: 42 });
        assert_eq!(format_text(&msg), Some("Context compaction started (42 messages)".to_string()));
    }

    #[test]
    fn format_text_formats_context_compaction_result() {
        let msg = AgentEvent::Context(ContextEvent::CompactionResult {
            summary: "summary here".to_string(),
            messages_removed: 10,
        });
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
        assert_eq!(format_text(&AgentEvent::Context(ContextEvent::Cleared)), Some("Context cleared".to_string()));
    }

    #[test]
    fn event_kind_none_for_non_output_fragments() {
        assert_eq!(event_kind(&AgentEvent::text("id", "x", false, "m")), None);
        assert_eq!(event_kind(&AgentEvent::thought("id", "x", false, "m")), None);
        assert_eq!(
            event_kind(&AgentEvent::Tool(ToolEvent::CallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
                model_name: "m".to_string(),
            })),
            None,
        );
    }

    #[test]
    fn event_kind_turn_ended_is_filterable() {
        assert_eq!(
            event_kind(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed })),
            Some(CliEventKind::TurnEnded)
        );
    }

    #[test]
    fn should_emit_empty_filter_rejects_non_output_events() {
        assert!(should_emit(&tool_call_msg(), &[]));
        assert!(should_emit(
            &AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "e".to_string() } }),
            &[]
        ));
        assert!(should_emit(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }), &[]));
        assert!(!should_emit(&AgentEvent::text("id", "x", false, "m"), &[]));
        assert!(!should_emit(
            &AgentEvent::Tool(ToolEvent::CallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
                model_name: "m".to_string(),
            }),
            &[],
        ));
    }

    #[test]
    fn should_emit_single_type_whitelist() {
        let filter = &[CliEventKind::ToolCall];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(!should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }), filter));
    }

    #[test]
    fn should_emit_multi_type_whitelist() {
        let filter = &[CliEventKind::ToolCall, CliEventKind::ToolResult];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }), filter));
    }

    #[test]
    fn should_emit_turn_ended_respects_filter() {
        let msg = AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed });
        assert!(should_emit(&msg, &[CliEventKind::TurnEnded]));
        assert!(!should_emit(&msg, &[CliEventKind::ToolCall]));
    }

    #[test]
    fn cli_event_kind_names_match_agent_event_type_tags() {
        use clap::ValueEnum;

        let samples = vec![
            (AgentEvent::text("id", "x", true, "m"), CliEventKind::Text),
            (AgentEvent::thought("id", "x", true, "m"), CliEventKind::Thought),
            (tool_call_msg(), CliEventKind::ToolCall),
            (tool_result_msg(), CliEventKind::ToolResult),
            (
                AgentEvent::Tool(ToolEvent::Error {
                    error: llm::ToolCallError {
                        id: "tc1".to_string(),
                        name: "bash".to_string(),
                        arguments: None,
                        error: "boom".to_string(),
                    },
                    model_name: "test".to_string(),
                }),
                CliEventKind::ToolError,
            ),
            (AgentEvent::Turn(TurnEvent::AutoContinue { attempt: 1, max_attempts: 3 }), CliEventKind::AutoContinue),
            (
                AgentEvent::Model(ModelEvent::Switched { previous: "a".to_string(), new: "b".to_string() }),
                CliEventKind::ModelSwitched,
            ),
            (tool_progress(1.0, None, None), CliEventKind::ToolProgress),
            (
                AgentEvent::Context(ContextEvent::CompactionStarted { message_count: 1 }),
                CliEventKind::ContextCompactionStarted,
            ),
            (
                AgentEvent::Context(ContextEvent::CompactionResult { summary: "s".to_string(), messages_removed: 1 }),
                CliEventKind::ContextCompactionResult,
            ),
            (usage_update(), CliEventKind::ContextUsage),
            (AgentEvent::Context(ContextEvent::Cleared), CliEventKind::ContextCleared),
            (AgentEvent::Turn(TurnEvent::Started { content: vec![] }), CliEventKind::TurnStarted),
            (AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }), CliEventKind::TurnEnded),
            (llm_call_started(0, None), CliEventKind::LlmCallStarted),
            (
                AgentEvent::Turn(TurnEvent::LlmCallEnded {
                    purpose: aether_core::events::LlmCallPurpose::Chat,
                    outcome: aether_core::events::LlmCallOutcome::Cancelled,
                }),
                CliEventKind::LlmCallEnded,
            ),
            (
                AgentEvent::Tool(ToolEvent::ExecutionStarted {
                    tool_id: "tc1".to_string(),
                    tool_name: "bash".to_string(),
                }),
                CliEventKind::ToolExecutionStarted,
            ),
            (AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools: vec![] }), CliEventKind::ToolDefinitionsUpdated),
        ];

        for kind in CliEventKind::value_variants() {
            assert!(samples.iter().any(|(_, k)| k == kind), "samples is missing a case for {kind:?}");
        }

        for (msg, kind) in &samples {
            assert_eq!(event_kind(msg), Some(*kind), "event_kind disagrees for {kind:?}");
        }
    }

    #[tokio::test]
    async fn stream_output_turn_ended_breaks_loop_under_filter() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed })).await.unwrap();
        let filter = vec![CliEventKind::ToolCall];
        let code = stream_output(rx, OutputFormat::Text, &filter).await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[tokio::test]
    async fn stream_output_failed_turn_exits_nonzero() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "boom".to_string() } }))
            .await
            .unwrap();
        let code = stream_output(rx, OutputFormat::Text, &[]).await;
        assert_eq!(code, ExitCode::FAILURE);
    }

    fn tool_call_msg() -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            model_name: "test".to_string(),
        })
    }

    fn tool_result_msg() -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        })
    }

    fn tool_progress(progress: f64, total: Option<f64>, message: Option<&str>) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Progress {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            progress,
            total,
            message: message.map(str::to_string),
        })
    }

    fn llm_call_started(attempt: u32, delay_ms: Option<u64>) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: aether_core::events::LlmCallPurpose::Chat,
            provider: None,
            model: None,
            display_name: "test".to_string(),
            attempt,
            max_attempts: 3,
            delay_ms,
        })
    }

    fn usage_update() -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: ContextUsage {
                input_tokens: 1500,
                output_tokens: 250,
                total_input_tokens: 5000,
                total_output_tokens: 800,
                ..Default::default()
            },
        })
    }
}
