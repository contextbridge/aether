use aether_core::core::{AgentDeps, Prompt, recv_agent_event};
use aether_core::events::{
    AgentEvent, Command, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome,
};
use aether_telemetry::TelemetryRuntime;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::telemetry::build_telemetry_runtime;

use super::{CliEventKind, RunConfig};
use crate::error::CliError;
use crate::output::{OutputFormat, print_message};
use crate::runtime::RuntimeBuilder;
use crate::slash_commands::expand_slash_command_text;

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
    let mut agent = RuntimeBuilder::from_spec(config.cwd.clone(), spec)
        .mcp_config(config.mcp_config)
        .agent_deps(deps)
        .wait_for_mcp()
        .build(Vec::new())
        .await?;

    let prompt = expand_slash_command_text(&agent.mcp, config.prompt).await;

    let outcome = match agent.agent_tx.send(Command::text(&prompt)).await {
        Ok(()) => Ok(stream_output(&mut agent.agent_rx, config.output, &config.events).await),
        Err(_) => Err(CliError::AgentStopped),
    };

    agent.shutdown().await;

    outcome
}

async fn stream_output(rx: &mut mpsc::Receiver<AgentEvent>, format: OutputFormat, events: &[CliEventKind]) -> ExitCode {
    loop {
        let msg = recv_agent_event(rx).await;
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
        AgentEvent::Tool(ToolEvent::DefinitionsUpdated { .. }) => Some(CliEventKind::ToolDefinitionsUpdated),
        AgentEvent::Message(
            MessageEvent::Text { is_complete: false, .. } | MessageEvent::Thought { is_complete: false, .. },
        )
        | AgentEvent::Turn(TurnEvent::UserMessageInserted { .. } | TurnEvent::UserMessageDiscarded { .. })
        | AgentEvent::Tool(ToolEvent::InputStarted { .. } | ToolEvent::InputDelta { .. }) => None,
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
    use aether_core::events::StreamState;

    use super::*;
    use crate::output::test_events::{llm_call_started, retry_scheduled, tool_progress, usage_update};

    #[test]
    fn should_emit_skips_fragments_and_applies_the_event_filter() {
        let call = AgentEvent::tool_call("tc1", "bash", "{}");
        let result = AgentEvent::tool_result("tc1", "bash", "{}", "ok");
        let done = AgentEvent::turn_ended(TurnOutcome::Completed);
        let calls = &[CliEventKind::ToolCall][..];
        let calls_and_results = &[CliEventKind::ToolCall, CliEventKind::ToolResult][..];
        let cases: [(AgentEvent, &[CliEventKind], bool); 13] = [
            (call.clone(), &[], true),
            (AgentEvent::turn_ended(TurnOutcome::failed("e")), &[], true),
            (done.clone(), &[], true),
            (AgentEvent::text("id", "x", StreamState::Partial), &[], false),
            (AgentEvent::thought("id", "x", StreamState::Partial), &[], false),
            (AgentEvent::Tool(ToolEvent::InputStarted { id: "tc1".into(), name: "bash".into() }), &[], false),
            (AgentEvent::Tool(ToolEvent::InputDelta { id: "tc1".into(), chunk: "x".into() }), &[], false),
            (result.clone(), calls, false),
            (done.clone(), calls, false),
            (call, calls_and_results, true),
            (result, calls_and_results, true),
            (done.clone(), calls_and_results, false),
            (done, &[CliEventKind::TurnEnded], true),
        ];
        for (event, filter, expected) in cases {
            assert_eq!(should_emit(&event, filter), expected, "{event:?} with filter {filter:?}");
        }
    }

    #[test]
    fn event_kind_covers_every_cli_event_kind() {
        use clap::ValueEnum;

        let samples = vec![
            (AgentEvent::text("id", "x", StreamState::Complete), CliEventKind::Text),
            (AgentEvent::thought("id", "x", StreamState::Complete), CliEventKind::Thought),
            (AgentEvent::tool_call("tc1", "bash", "{}"), CliEventKind::ToolCall),
            (AgentEvent::tool_result("tc1", "bash", "{}", "ok"), CliEventKind::ToolResult),
            (AgentEvent::tool_error("tc1", "bash", "{}", "boom"), CliEventKind::ToolError),
            (
                AgentEvent::Turn(TurnEvent::AutoContinue {
                    attempt: 1,
                    max_attempts: 3,
                    message_id: llm::MessageId::new(),
                    content: vec![],
                }),
                CliEventKind::AutoContinue,
            ),
            (
                AgentEvent::Model(ModelEvent::Switched { previous: "a".to_string(), new: "b".to_string() }),
                CliEventKind::ModelSwitched,
            ),
            (tool_progress(1.0, None, None), CliEventKind::ToolProgress),
            (
                AgentEvent::Context(ContextEvent::CompactionStarted {
                    compaction_id: "compaction".into(),
                    message_count: 1,
                }),
                CliEventKind::ContextCompactionStarted,
            ),
            (
                AgentEvent::Context(ContextEvent::CompactionEnded {
                    compaction_id: "compaction".into(),
                    outcome: aether_core::events::CompactionOutcome::Completed,
                }),
                CliEventKind::ContextCompactionEnded,
            ),
            (
                AgentEvent::Context(ContextEvent::CompactionResult {
                    compaction_id: "compaction".into(),
                    message_id: llm::MessageId::new(),
                    summary: "s".to_string(),
                    messages_removed: 1,
                }),
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
    async fn stream_output_exit_code_follows_the_turn_outcome_or_closed_channel() {
        let cases = [
            (AgentEvent::turn_ended(TurnOutcome::Completed), &[CliEventKind::ToolCall][..], ExitCode::SUCCESS),
            (AgentEvent::turn_ended(TurnOutcome::failed("boom")), &[], ExitCode::FAILURE),
            (AgentEvent::Turn(TurnEvent::Started { content: vec![] }), &[], ExitCode::FAILURE),
        ];
        for (event, filter, expected) in cases {
            let (tx, mut rx) = mpsc::channel(1);
            tx.send(event).await.unwrap();
            drop(tx);
            assert_eq!(stream_output(&mut rx, OutputFormat::Text, filter).await, expected);
        }
    }
}
