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
            TurnOutcome::Failed { error } => format!("Error: {error}"),
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
            let bar = match total {
                Some(total) => format!("{progress}/{total}"),
                None => format!("{progress}"),
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::StreamState;

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
        let message = AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"cmd":"ls"}"#.to_string(),
            },
        });
        assert_eq!(format_text(&message), Some(r#"Tool call: bash({"cmd":"ls"})"#.to_string()));
    }

    #[test]
    fn format_text_skips_tool_call_updates() {
        let message =
            AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: "tc1".to_string(), chunk: "partial".to_string() });
        assert_eq!(format_text(&message), None);
    }

    #[test]
    fn format_text_formats_tool_result() {
        assert_eq!(format_text(&tool_result()), Some("Tool result [bash]: ok".to_string()));
    }

    #[test]
    fn format_text_formats_tool_error() {
        let message = AgentEvent::Tool(ToolEvent::Error {
            error: llm::ToolCallError {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: None,
                error: "not found".to_string(),
            },
        });
        assert_eq!(format_text(&message), Some("Tool error [bash]: not found".to_string()));
    }

    #[test]
    fn format_text_formats_turn_outcomes() {
        assert_eq!(
            format_text(&AgentEvent::Turn(TurnEvent::Ended {
                outcome: TurnOutcome::Failed { error: "boom".to_string() }
            })),
            Some("Error: boom".to_string())
        );
        assert_eq!(format_text(&AgentEvent::turn_ended(TurnOutcome::Cancelled)), Some("Cancelled".to_string()));
        assert_eq!(format_text(&AgentEvent::turn_ended(TurnOutcome::Completed)), Some("Done".to_string()));
    }

    #[test]
    fn format_text_formats_retry_events() {
        let started = AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: "test".to_string(),
            attempt: 0,
            max_attempts: 3,
        });
        assert_eq!(format_text(&started), None);
        assert_eq!(format_text(&retry_scheduled()), Some("Retrying (1/3) in 10ms".to_string()));
        let retrying = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: llm::LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::failed("overloaded", true),
        });
        assert_eq!(format_text(&retrying), Some("LLM call failed (will retry): overloaded".to_string()));
        let terminal = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: llm::LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::failed("boom", false),
        });
        assert_eq!(format_text(&terminal), None);
    }

    #[test]
    fn format_text_formats_auto_continue_and_model_switch() {
        let continuing = AgentEvent::Turn(TurnEvent::AutoContinue {
            attempt: 2,
            max_attempts: 5,
            message_id: llm::MessageId::new(),
            content: vec![],
        });
        assert_eq!(format_text(&continuing), Some("Continuing (2/5)...".to_string()));
        let switched =
            AgentEvent::Model(ModelEvent::Switched { previous: "old-model".to_string(), new: "new-model".to_string() });
        assert_eq!(format_text(&switched), Some("Model switched: old-model -> new-model".to_string()));
    }

    #[test]
    fn format_text_formats_tool_progress() {
        assert_eq!(
            format_text(&tool_progress(50.0, Some(100.0), Some("halfway"))),
            Some("Tool progress [bash]: 50/100 - halfway".to_string())
        );
        assert_eq!(format_text(&tool_progress(42.0, None, None)), Some("Tool progress [bash]: 42".to_string()));
    }

    #[test]
    fn format_text_formats_context_events() {
        let started = AgentEvent::Context(ContextEvent::CompactionStarted {
            compaction_id: "compaction".into(),
            message_count: 42,
        });
        assert_eq!(format_text(&started), Some("Context compaction started (42 messages)".to_string()));
        let result = AgentEvent::Context(ContextEvent::CompactionResult {
            compaction_id: "compaction".into(),
            message_id: llm::MessageId::new(),
            summary: "summary here".to_string(),
            messages_removed: 10,
        });
        assert_eq!(format_text(&result), Some("Context compacted: 10 messages removed. summary here".to_string()));
        assert_eq!(format_text(&usage_update()), Some("Context: 100000 / 200000 tokens (50.0%)".to_string()));
        assert_eq!(format_text(&AgentEvent::Context(ContextEvent::Cleared)), Some("Context cleared".to_string()));
    }

    fn tool_result() -> AgentEvent {
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

    fn retry_scheduled() -> AgentEvent {
        AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: llm::LlmCallPurpose::Chat,
            attempt: 1,
            max_attempts: 3,
            delay_ms: 10,
        })
    }

    fn usage_update() -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: llm::ContextUsage {
                input_tokens: 100_000.into(),
                context_limit: Some(200_000.into()),
                usage_ratio: Some(0.5),
            },
        })
    }
}
