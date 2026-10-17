use aether_core::events::{
    AgentEvent, CompactionOutcome, ContextEvent, LlmCallOutcome, MessageEvent, ModelEvent, ToolEvent, TurnEvent,
    TurnOutcome,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Pretty,
    Json,
}

pub(crate) fn print_message(format: OutputFormat, message: &AgentEvent) -> Result<(), serde_json::Error> {
    match format {
        OutputFormat::Text => {
            if let Some(text) = format_text(message) {
                if matches!(message, AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } })) {
                    eprintln!("{text}");
                } else {
                    println!("{text}");
                }
            }
        }
        OutputFormat::Pretty => println!("{}", serde_json::to_string_pretty(message)?),
        OutputFormat::Json => println!("{}", serde_json::to_string(message)?),
    }

    Ok(())
}

fn format_text(message: &AgentEvent) -> Option<String> {
    match message {
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
            TurnOutcome::Failed { error, .. } => format!("Error: {error}"),
        }),
        AgentEvent::Turn(TurnEvent::AutoContinue { attempt, max_attempts, .. }) => {
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
            let bar = total.map_or_else(|| progress.to_string(), |total| format!("{progress}/{total}"));
            let suffix = message.as_deref().map(|message| format!(" - {message}")).unwrap_or_default();
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
        AgentEvent::Context(ContextEvent::CompactionStarted { message_count, .. }) => {
            Some(format!("Context compaction started ({message_count} messages)"))
        }
        AgentEvent::Context(ContextEvent::CompactionEnded { outcome, .. }) => Some(match outcome {
            CompactionOutcome::Completed => "Context compaction completed".to_string(),
            CompactionOutcome::Failed { error } => format!("Context compaction failed: {error}"),
            CompactionOutcome::Cancelled => "Context compaction cancelled".to_string(),
        }),
        AgentEvent::Context(ContextEvent::CompactionResult { summary, messages_removed, .. }) => {
            Some(format!("Context compacted: {messages_removed} messages removed. {summary}"))
        }
        AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => Some(format_context_usage(usage)),
        AgentEvent::Context(ContextEvent::Cleared) => Some("Context cleared".to_string()),
        AgentEvent::SessionUsage(usage) => Some(format_session_usage(usage)),
        AgentEvent::Turn(
            TurnEvent::Started { .. }
            | TurnEvent::UserMessageInserted { .. }
            | TurnEvent::UserMessageDiscarded { .. }
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
            ToolEvent::InputStarted { .. } | ToolEvent::InputDelta { .. } | ToolEvent::DefinitionsUpdated { .. },
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

#[cfg(test)]
pub(crate) mod test_events {
    use aether_core::events::{AgentEvent, ContextEvent, ToolEvent, TurnEvent};

    pub(crate) fn tool_progress(progress: f64, total: Option<f64>, message: Option<&str>) -> AgentEvent {
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

    pub(crate) fn llm_call_started(attempt: u32) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: "test".to_string(),
            attempt,
            max_attempts: 3,
        })
    }

    pub(crate) fn retry_scheduled(attempt: u32, delay_ms: u64) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: llm::LlmCallPurpose::Chat,
            attempt,
            max_attempts: 3,
            delay_ms,
        })
    }

    pub(crate) fn usage_update() -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: llm::ContextUsage {
                input_tokens: 100_000.into(),
                context_limit: Some(200_000.into()),
                usage_ratio: Some(0.5),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_events::{llm_call_started, retry_scheduled, tool_progress, usage_update};
    use super::*;
    use aether_core::events::StreamState;

    #[test]
    fn format_text_renders_or_skips_each_event() {
        let llm_call_failed = |error, will_retry| {
            AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: llm::LlmCallPurpose::Chat,
                outcome: LlmCallOutcome::failed(error, will_retry),
            })
        };
        let auto_continue = AgentEvent::Turn(TurnEvent::AutoContinue {
            attempt: 2,
            max_attempts: 5,
            message_id: llm::MessageId::new(),
            content: vec![],
        });
        let switched =
            AgentEvent::Model(ModelEvent::Switched { previous: "old-model".into(), new: "new-model".into() });
        let compaction_started = AgentEvent::Context(ContextEvent::CompactionStarted {
            compaction_id: "compaction".into(),
            message_count: 42,
        });
        let compacted = AgentEvent::Context(ContextEvent::CompactionResult {
            compaction_id: "compaction".into(),
            message_id: llm::MessageId::new(),
            summary: "summary here".into(),
            messages_removed: 10,
        });
        let cases = [
            (AgentEvent::text("id", "hello world", StreamState::Complete), Some("hello world")),
            (AgentEvent::text("id", "partial", StreamState::Partial), None),
            (AgentEvent::thought("id", "reasoning here", StreamState::Complete), Some("Thought: reasoning here")),
            (AgentEvent::thought("id", "partial", StreamState::Partial), None),
            (AgentEvent::tool_call("tc1", "bash", r#"{"cmd":"ls"}"#), Some(r#"Tool call: bash({"cmd":"ls"})"#)),
            (AgentEvent::Tool(ToolEvent::InputStarted { id: "tc1".into(), name: "bash".into() }), None),
            (AgentEvent::Tool(ToolEvent::InputDelta { id: "tc1".into(), chunk: "partial".into() }), None),
            (AgentEvent::tool_result("tc1", "bash", "{}", "ok"), Some("Tool result [bash]: ok")),
            (AgentEvent::tool_error("tc1", "bash", "{}", "not found"), Some("Tool error [bash]: not found")),
            (tool_progress(50.0, Some(100.0), Some("halfway")), Some("Tool progress [bash]: 50/100 - halfway")),
            (tool_progress(42.0, None, None), Some("Tool progress [bash]: 42")),
            (AgentEvent::turn_ended(TurnOutcome::failed("boom")), Some("Error: boom")),
            (AgentEvent::turn_ended(TurnOutcome::Cancelled), Some("Cancelled")),
            (AgentEvent::turn_ended(TurnOutcome::Completed), Some("Done")),
            (llm_call_started(0), None),
            (retry_scheduled(1, 10), Some("Retrying (1/3) in 10ms")),
            (llm_call_failed("overloaded", true), Some("LLM call failed (will retry): overloaded")),
            (llm_call_failed("boom", false), None),
            (auto_continue, Some("Continuing (2/5)...")),
            (switched, Some("Model switched: old-model -> new-model")),
            (compaction_started, Some("Context compaction started (42 messages)")),
            (compacted, Some("Context compacted: 10 messages removed. summary here")),
            (usage_update(), Some("Context: 100000 / 200000 tokens (50.0%)")),
            (AgentEvent::Context(ContextEvent::Cleared), Some("Context cleared")),
        ];
        for (event, expected) in cases {
            assert_eq!(format_text(&event).as_deref(), expected, "{event:?}");
        }
    }
}
