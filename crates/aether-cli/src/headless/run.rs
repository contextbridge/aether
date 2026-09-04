use aether_core::core::{AgentDeps, Prompt};
use aether_core::events::{
    AgentEvent, Command, CompactionOutcome, ContextEvent, LlmCallOutcome, MessageEvent, ModelEvent, ToolEvent,
    TurnEvent, TurnOutcome,
};
use aether_core::mcp::McpHandle;
use aether_telemetry::TelemetryRuntime;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::error;

use crate::telemetry::build_telemetry_runtime;

use super::error::CliError;
use super::{CliEventKind, RunConfig};
use crate::output::OutputFormat;
use crate::runtime::RuntimeBuilder;
use crate::slash_commands::{expand_slash_command, parse_slash_command};

pub async fn run(config: RunConfig) -> Result<ExitCode, CliError> {
    setup_tracing(config.verbose);

    let telemetry = build_telemetry_runtime(config.telemetry.as_ref(), config.trace_context.clone())?;
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

    let registry = config.agent_catalog.registry().clone();
    let deps =
        AgentDeps::new(config.oauth_credential_store, telemetry.as_ref().map(|runtime| runtime.observer_factory()))
            .with_agent_registry(registry);
    let (agent, _mcp_snapshot) = RuntimeBuilder::from_spec(config.cwd.clone(), spec)
        .mcp_sources(config.mcp_config_sources)
        .agent_deps(deps)
        .build_ready(vec![])
        .await?;

    let prompt = expand_prompt(agent.mcp_runtime.handle(), config.prompt).await;

    agent
        .agent_tx
        .send(Command::text(&prompt))
        .await
        .map_err(|e| CliError::AgentError(format!("Failed to send prompt: {e}")))?;

    let exit_code = stream_output(agent.agent_rx, config.output, &config.events).await;

    drop(agent.agent_tx);
    agent.agent_handle.await_completion().await;

    Ok(exit_code)
}

async fn expand_prompt(mcp: &McpHandle, prompt: String) -> String {
    let Some(slash_command) = parse_slash_command(&prompt) else {
        return prompt;
    };

    match expand_slash_command(mcp, slash_command.command_name, slash_command.args_text).await {
        Ok(expanded) => expanded,
        Err(error) => {
            error!("Failed to expand slash command: {error}");
            prompt
        }
    }
}

async fn stream_output(mut rx: mpsc::Receiver<AgentEvent>, format: OutputFormat, events: &[CliEventKind]) -> ExitCode {
    while let Some(msg) = rx.recv().await {
        if should_emit(&msg, events)
            && let Err(error) = print_message(format, &msg)
        {
            eprintln!("Failed to serialize headless event: {error}");
            return ExitCode::FAILURE;
        }

        if let Some(outcome) = msg.turn_outcome() {
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
        AgentEvent::Tool(
            ToolEvent::Result { .. } | ToolEvent::TaskCreated { .. } | ToolEvent::TaskCompleted { .. },
        ) => Some(CliEventKind::ToolResult),
        AgentEvent::Tool(ToolEvent::Error { .. } | ToolEvent::TaskFailed { .. } | ToolEvent::TaskCancelled { .. }) => {
            Some(CliEventKind::ToolError)
        }
        AgentEvent::Turn(TurnEvent::AutoContinue { .. }) => Some(CliEventKind::AutoContinue),
        AgentEvent::Model(ModelEvent::Switched { .. }) => Some(CliEventKind::ModelSwitched),
        AgentEvent::Tool(
            ToolEvent::Progress { .. }
            | ToolEvent::DisplayUpdate { .. }
            | ToolEvent::SubAgentProgress { .. }
            | ToolEvent::TaskStatus { .. },
        ) => Some(CliEventKind::ToolProgress),
        AgentEvent::Context(ContextEvent::CompactionStarted { .. }) => Some(CliEventKind::ContextCompactionStarted),
        AgentEvent::Context(ContextEvent::CompactionEnded { .. }) => Some(CliEventKind::ContextCompactionEnded),
        AgentEvent::Context(ContextEvent::CompactionResult { .. }) => Some(CliEventKind::ContextCompactionResult),
        AgentEvent::Context(ContextEvent::UsageUpdated { .. }) => Some(CliEventKind::ContextUsage),
        AgentEvent::SessionUsage(_) => Some(CliEventKind::SessionUsage),
        AgentEvent::Context(ContextEvent::Cleared) => Some(CliEventKind::ContextCleared),
        AgentEvent::Turn(TurnEvent::Started { .. }) => Some(CliEventKind::TurnStarted),
        AgentEvent::Turn(TurnEvent::Ended { .. }) => Some(CliEventKind::TurnEnded),
        AgentEvent::Turn(TurnEvent::RetryScheduled { .. }) => Some(CliEventKind::LlmRetryScheduled),
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

        AgentEvent::Tool(ToolEvent::TaskStatus { request, task_id, status, status_message }) => Some(format!(
            "Task status [{}]: {} {}{}",
            request.name,
            task_id,
            status,
            status_message.as_deref().map(|message| format!(" - {message}")).unwrap_or_default()
        )),
        AgentEvent::Tool(ToolEvent::TaskCreated { request, task_id, .. }) => {
            Some(format!("Tool deferred [{}]: task {}", request.name, task_id))
        }
        AgentEvent::Tool(ToolEvent::TaskCompleted { request, task_id, result, .. }) => {
            Some(format!("Background task completed [{}]: {}: {}", request.name, task_id, result.result))
        }
        AgentEvent::Tool(ToolEvent::TaskFailed { request, task_id, error, .. }) => {
            Some(format!("Background task failed [{}]: {}: {}", request.name, task_id, error.error))
        }
        AgentEvent::Tool(ToolEvent::TaskCancelled { request, task_id, .. }) => {
            Some(format!("Background task cancelled [{}]: {task_id}", request.name))
        }
        AgentEvent::Turn(TurnEvent::Ended { outcome }) => Some(match outcome {
            TurnOutcome::Completed => "Done".to_string(),
            TurnOutcome::Cancelled => "Cancelled".to_string(),
            TurnOutcome::Failed { error } => format!("Error: {error}"),
        }),

        AgentEvent::Turn(TurnEvent::AutoContinue { attempt, max_attempts }) => {
            Some(format!("Continuing ({attempt}/{max_attempts})..."))
        }

        AgentEvent::Turn(event @ TurnEvent::RetryScheduled { .. }) => event
            .retry_info()
            .map(|retry| format!("Retrying ({}/{}) in {}ms", retry.attempt, retry.max_attempts, retry.delay_ms)),

        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            outcome: LlmCallOutcome::Failed { error, will_retry: true, .. },
            ..
        }) => Some(format!("LLM call failed (will retry): {error}")),

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

        AgentEvent::Tool(ToolEvent::DisplayUpdate { request, meta }) => {
            Some(format!("Tool progress [{}]: {} - {}", request.name, meta.display.title, meta.display.value))
        }

        AgentEvent::Tool(ToolEvent::SubAgentProgress { payload, .. }) => match &payload.event {
            AgentEvent::SessionUsage(_) => None,
            event => {
                format_text(event).map(|text| format!("Sub-agent {} [{}]: {text}", payload.agent_name, payload.task_id))
            }
        },

        AgentEvent::Context(ContextEvent::CompactionStarted { message_count }) => {
            Some(format!("Context compaction started ({message_count} messages)"))
        }

        AgentEvent::Context(ContextEvent::CompactionEnded { outcome }) => Some(match outcome {
            CompactionOutcome::Completed => "Context compaction completed".to_string(),
            CompactionOutcome::Failed { error } => format!("Context compaction failed: {error}"),
            CompactionOutcome::Cancelled => "Context compaction cancelled".to_string(),
        }),

        AgentEvent::Context(ContextEvent::CompactionResult { summary, messages_removed }) => {
            Some(format!("Context compacted: {messages_removed} messages removed. {summary}"))
        }

        AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => Some(format_context_usage(usage)),

        AgentEvent::Context(ContextEvent::Cleared) => Some("Context cleared".to_string()),

        AgentEvent::SessionUsage(usage) => Some(format_session_usage(usage)),

        AgentEvent::Turn(
            TurnEvent::Started { .. }
            | TurnEvent::LlmCallStarted { .. }
            | TurnEvent::LlmCallEnded {
                outcome:
                    LlmCallOutcome::Completed { .. }
                    | LlmCallOutcome::Cancelled
                    | LlmCallOutcome::Failed { will_retry: false, .. },
                ..
            },
        )
        | AgentEvent::Tool(
            ToolEvent::ExecutionStarted { .. } | ToolEvent::DefinitionsUpdated { .. } | ToolEvent::CallUpdate { .. },
        )
        | AgentEvent::Message(MessageEvent::Text { .. } | MessageEvent::Thought { .. }) => None,
    }
}

fn format_context_usage(usage: &llm::ContextUsage) -> String {
    match (usage.context_limit, usage.usage_ratio) {
        (Some(limit), Some(ratio)) => {
            format!("Context: {} / {limit} tokens ({:.1}%)", usage.input_tokens, ratio * 100.0)
        }
        _ => format!("Context: {} tokens", usage.input_tokens),
    }
}

fn format_session_usage(usage: &llm::SessionUsageEvent) -> String {
    let call_cost =
        usage.estimated_cost.map_or_else(|| "unknown".to_string(), |cost| format!("${:.6}", cost.total_usd));
    let totals = &usage.totals;
    let cumulative_cost = if totals.is_fully_priced() {
        format!("estimated total: ${:.6}", totals.estimated_usd)
    } else {
        format!("known subtotal: ${:.6}, {} unpriced calls", totals.estimated_usd, totals.unpriced_calls)
    };
    format!(
        "Session usage #{} [{}]: {} in, {} out (call cost: {}, cumulative: {} tokens, {})",
        usage.sequence,
        usage.source.agent_name,
        usage.tokens.input_tokens,
        usage.tokens.output_tokens,
        call_cost,
        totals.tokens.total_tokens(),
        cumulative_cost,
    )
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
    use aether_core::events::StreamState;

    use super::*;
    use llm::ContextUsage;

    #[test]
    fn format_text_formats_complete_text() {
        assert_eq!(
            format_text(&AgentEvent::text("id", "hello world", StreamState::Complete)),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn format_text_skips_incomplete_text() {
        assert_eq!(format_text(&AgentEvent::text("id", "partial", StreamState::Partial)), None);
    }

    #[test]
    fn format_text_formats_complete_thought() {
        assert_eq!(
            format_text(&AgentEvent::thought("id", "reasoning here", StreamState::Complete)),
            Some("Thought: reasoning here".to_string())
        );
    }

    #[test]
    fn format_text_skips_incomplete_thought() {
        assert_eq!(format_text(&AgentEvent::thought("id", "partial", StreamState::Partial)), None);
    }

    #[test]
    fn format_text_formats_tool_call() {
        let msg = AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"cmd":"ls"}"#.to_string(),
            },
        });
        assert_eq!(format_text(&msg), Some(r#"Tool call: bash({"cmd":"ls"})"#.to_string()));
    }

    #[test]
    fn format_text_skips_tool_call_updates() {
        let msg =
            AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: "tc1".to_string(), chunk: "partial".to_string() });
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
        let msg = AgentEvent::turn_ended(TurnOutcome::Cancelled);
        assert_eq!(format_text(&msg), Some("Cancelled".to_string()));
    }

    #[test]
    fn format_text_formats_retry_schedule() {
        let msg = retry_scheduled(1, 10);
        assert_eq!(format_text(&msg), Some("Retrying (1/3) in 10ms".to_string()));
    }

    #[test]
    fn format_text_formats_llm_call_failure_that_will_retry() {
        let msg = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: llm::LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::failed("overloaded", true),
        });
        assert_eq!(format_text(&msg), Some("LLM call failed (will retry): overloaded".to_string()));
    }

    #[test]
    fn format_text_skips_terminal_llm_call_failure() {
        let msg = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: llm::LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::failed("boom", false),
        });
        assert_eq!(format_text(&msg), None);
    }

    #[test]
    fn format_text_skips_first_call_start() {
        assert_eq!(format_text(&llm_call_started(0)), None);
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
        assert_eq!(format_text(&AgentEvent::turn_ended(TurnOutcome::Completed)), Some("Done".to_string()));
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
        assert_eq!(format_text(&usage_update()), Some("Context: 100000 / 200000 tokens (50.0%)".to_string()));
    }

    #[test]
    fn format_text_formats_context_cleared() {
        assert_eq!(format_text(&AgentEvent::Context(ContextEvent::Cleared)), Some("Context cleared".to_string()));
    }

    #[test]
    fn event_kind_none_for_non_output_fragments() {
        assert_eq!(event_kind(&AgentEvent::text("id", "x", StreamState::Partial)), None);
        assert_eq!(event_kind(&AgentEvent::thought("id", "x", StreamState::Partial)), None);
        assert_eq!(
            event_kind(&AgentEvent::Tool(ToolEvent::CallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
            })),
            None,
        );
    }

    #[test]
    fn event_kind_turn_ended_is_filterable() {
        assert_eq!(event_kind(&AgentEvent::turn_ended(TurnOutcome::Completed)), Some(CliEventKind::TurnEnded));
    }

    #[test]
    fn should_emit_empty_filter_rejects_non_output_events() {
        assert!(should_emit(&tool_call_msg(), &[]));
        assert!(should_emit(
            &AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "e".to_string() } }),
            &[]
        ));
        assert!(should_emit(&AgentEvent::turn_ended(TurnOutcome::Completed), &[]));
        assert!(!should_emit(&AgentEvent::text("id", "x", StreamState::Partial), &[]));
        assert!(!should_emit(
            &AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: "tc1".to_string(), chunk: "x".to_string() }),
            &[],
        ));
    }

    #[test]
    fn should_emit_single_type_whitelist() {
        let filter = &[CliEventKind::ToolCall];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(!should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentEvent::turn_ended(TurnOutcome::Completed), filter));
    }

    #[test]
    fn should_emit_multi_type_whitelist() {
        let filter = &[CliEventKind::ToolCall, CliEventKind::ToolResult];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentEvent::turn_ended(TurnOutcome::Completed), filter));
    }

    #[test]
    fn should_emit_turn_ended_respects_filter() {
        let msg = AgentEvent::turn_ended(TurnOutcome::Completed);
        assert!(should_emit(&msg, &[CliEventKind::TurnEnded]));
        assert!(!should_emit(&msg, &[CliEventKind::ToolCall]));
    }

    #[test]
    fn event_kind_covers_every_cli_event_kind() {
        use clap::ValueEnum;

        let samples = vec![
            (AgentEvent::text("id", "x", StreamState::Complete), CliEventKind::Text),
            (AgentEvent::thought("id", "x", StreamState::Complete), CliEventKind::Thought),
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
                AgentEvent::Context(ContextEvent::CompactionEnded { outcome: CompactionOutcome::Completed }),
                CliEventKind::ContextCompactionEnded,
            ),
            (
                AgentEvent::Context(ContextEvent::CompactionResult { summary: "s".to_string(), messages_removed: 1 }),
                CliEventKind::ContextCompactionResult,
            ),
            (usage_update(), CliEventKind::ContextUsage),
            (
                AgentEvent::SessionUsage(llm::testing::session_usage_event(1, llm::TokenUsage::new(1, 1))),
                CliEventKind::SessionUsage,
            ),
            (AgentEvent::Context(ContextEvent::Cleared), CliEventKind::ContextCleared),
            (AgentEvent::Turn(TurnEvent::Started { content: vec![] }), CliEventKind::TurnStarted),
            (AgentEvent::turn_ended(TurnOutcome::Completed), CliEventKind::TurnEnded),
            (retry_scheduled(1, 10), CliEventKind::LlmRetryScheduled),
            (llm_call_started(0), CliEventKind::LlmCallStarted),
            (
                AgentEvent::Turn(TurnEvent::LlmCallEnded {
                    purpose: llm::LlmCallPurpose::Chat,
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
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let filter = vec![CliEventKind::ToolCall];
        let code = stream_output(rx, OutputFormat::Text, &filter).await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[tokio::test]
    async fn stream_output_failed_turn_exits_with_failure() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::turn_ended(TurnOutcome::Failed { error: "boom".to_string() })).await.unwrap();
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

    fn retry_scheduled(attempt: u32, delay_ms: u64) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: llm::LlmCallPurpose::Chat,
            attempt,
            max_attempts: 3,
            delay_ms,
        })
    }

    fn llm_call_started(attempt: u32) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: "test".to_string(),
            attempt,
            max_attempts: 3,
        })
    }

    fn usage_update() -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: ContextUsage {
                input_tokens: 100_000.into(),
                context_limit: Some(200_000.into()),
                usage_ratio: Some(0.5),
            },
        })
    }
}
