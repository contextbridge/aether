use acp_utils::notifications::{
    ContextClearedParams, ContextCompactionParams, SessionUsageParams, SubAgentEvent, SubAgentProgressParams,
    SubAgentToolCallUpdate, SubAgentToolError, SubAgentToolRequest, SubAgentToolResult,
};
use aether_core::events::{
    AgentEvent, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome, aether_tool_name_meta,
    humanize_tool_name, parse_tool_call_chunk,
};
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v2::{
    self as acp, Content, ContentBlock, ContentChunk, MessageId, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    SessionId, SessionUpdate, TextContent, ToolCallContent, ToolCallStatus, ToolCallUpdate, UpdateSessionNotification,
    UsageUpdate,
};
use agent_client_protocol::{JsonRpcMessage, UntypedMessage};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use mcp_utils::display_meta::{PlanMetaStatus, ToolResultMeta};

/// Converts Aether `AgentEvent` to ACP `SessionUpdate`
pub fn map_agent_event_to_session_notification(
    session_id: SessionId,
    msg: &AgentEvent,
) -> Option<UpdateSessionNotification> {
    map_agent_event_to_notification(session_id, msg, NotificationMode::Live)
}

pub fn map_replayed_agent_event(session_id: SessionId, msg: &AgentEvent) -> Option<UpdateSessionNotification> {
    map_agent_event_to_notification(session_id, msg, NotificationMode::Replay)
}

/// Typed union of agent-side extension notifications that the actor forwards
/// to the client. Each variant serializes to its own `_aether/*` wire method
/// and is sent via [`ConnectionTo<Client>::send_notification`].
pub enum AgentExtNotification {
    ContextCompaction(ContextCompactionParams),
    ContextCleared(ContextClearedParams),
    SubAgentProgress(Box<SubAgentProgressParams>),
    SessionUsage(Box<SessionUsageParams>),
}

impl AgentExtNotification {
    pub fn method(&self) -> &str {
        match self {
            Self::ContextCompaction(params) => params.method(),
            Self::ContextCleared(params) => params.method(),
            Self::SubAgentProgress(params) => params.method(),
            Self::SessionUsage(params) => params.method(),
        }
    }

    pub fn to_untyped(&self) -> Result<UntypedMessage, agent_client_protocol::Error> {
        match self {
            Self::ContextCompaction(params) => params.to_untyped_message(),
            Self::ContextCleared(params) => params.to_untyped_message(),
            Self::SubAgentProgress(params) => params.to_untyped_message(),
            Self::SessionUsage(params) => params.to_untyped_message(),
        }
    }
}

pub fn try_into_agent_notification(msg: &AgentEvent) -> Option<AgentExtNotification> {
    match msg {
        AgentEvent::Context(ContextEvent::CompactionStarted { .. }) => {
            Some(AgentExtNotification::ContextCompaction(ContextCompactionParams { active: true }))
        }
        AgentEvent::Context(ContextEvent::CompactionEnded { .. }) => {
            Some(AgentExtNotification::ContextCompaction(ContextCompactionParams { active: false }))
        }

        AgentEvent::Tool(ToolEvent::SubAgentProgress { request, payload }) => {
            Some(AgentExtNotification::SubAgentProgress(Box::new(SubAgentProgressParams {
                parent_tool_id: request.id.clone(),
                task_id: payload.task_id.clone(),
                agent_name: payload.agent_name.clone(),
                event: to_sub_agent_event(&payload.event),
            })))
        }
        AgentEvent::Context(ContextEvent::Cleared) => {
            Some(AgentExtNotification::ContextCleared(ContextClearedParams::default()))
        }
        AgentEvent::SessionUsage(usage) => {
            Some(AgentExtNotification::SessionUsage(Box::new(SessionUsageParams { usage: usage.clone() })))
        }
        _ => None,
    }
}

/// Replace the session's itemized plan with the tool result's plan snapshot.
pub fn try_extract_plan_notification(
    session_id: SessionId,
    result_meta: Option<&ToolResultMeta>,
) -> Option<UpdateSessionNotification> {
    let plan_meta = result_meta?.plan.as_ref()?;
    let entries = plan_meta
        .entries
        .iter()
        .map(|e| PlanEntry::new(e.content.clone(), PlanEntryPriority::Medium, plan_status_to_acp(e.status)))
        .collect();
    Some(UpdateSessionNotification::new(
        session_id,
        SessionUpdate::PlanUpdate(acp::PlanUpdate::new(acp::PlanUpdateContent::items("aether-plan", entries))),
    ))
}

#[derive(Clone, Copy)]
pub(crate) enum NotificationMode {
    Live,
    Replay,
}

pub(crate) fn map_agent_event_to_notification(
    session_id: SessionId,
    msg: &AgentEvent,
    mode: NotificationMode,
) -> Option<UpdateSessionNotification> {
    match msg {
        AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => {
            map_context_usage_to_notification(session_id, usage)
        }

        AgentEvent::Message(MessageEvent::Text { message_id, chunk, is_complete, .. }) => map_chunk_to_notification(
            session_id,
            chunk,
            *is_complete,
            mode,
            SessionUpdate::AgentMessageChunk,
            |id, content| SessionUpdate::AgentMessage(acp::AgentMessage::new(id).content(content)),
            message_id.as_str(),
        ),

        AgentEvent::Message(MessageEvent::Thought { message_id, chunk, is_complete, .. }) => map_chunk_to_notification(
            session_id,
            chunk,
            *is_complete,
            mode,
            SessionUpdate::AgentThoughtChunk,
            |id, content| SessionUpdate::AgentThought(acp::AgentThought::new(id).content(content)),
            message_id.as_str(),
        ),

        AgentEvent::Tool(ToolEvent::Call { request, .. }) => Some(map_tool_call_to_notification(session_id, request)),

        AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id, chunk, .. }) => {
            Some(map_tool_call_update_to_notification(session_id, tool_call_id, chunk))
        }

        AgentEvent::Tool(
            ToolEvent::Result { result, result_meta, .. } | ToolEvent::TaskCompleted { result, result_meta, .. },
        ) => Some(map_tool_result_to_notification(session_id, result, result_meta.as_ref())),

        AgentEvent::Tool(ToolEvent::Error { error, .. }) => Some(map_tool_error_to_notification(session_id, error)),

        AgentEvent::Tool(ToolEvent::TaskCreated { request, status_message, .. }) => {
            Some(UpdateSessionNotification::new(
                session_id,
                SessionUpdate::ToolCallUpdate(
                    ToolCallUpdate::new(request.id.clone())
                        .status(ToolCallStatus::Pending)
                        .title(status_message.as_deref().unwrap_or("Background task")),
                ),
            ))
        }

        AgentEvent::Tool(ToolEvent::TaskFailed { error, .. }) => {
            Some(map_tool_error_to_notification(session_id, error))
        }

        AgentEvent::Tool(ToolEvent::TaskCancelled { request, .. }) => Some(UpdateSessionNotification::new(
            session_id,
            SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(request.id.clone()).status(ToolCallStatus::Failed).content(vec![
                    ToolCallContent::Content(Box::new(Content::new(ContentBlock::Text(TextContent::new(
                        "The background task was cancelled and will not produce a result.",
                    ))))),
                ]),
            ),
        )),

        AgentEvent::Tool(ToolEvent::TaskStatus { request, status, status_message, .. }) => {
            Some(UpdateSessionNotification::new(
                session_id,
                SessionUpdate::ToolCallUpdate(
                    ToolCallUpdate::new(request.id.clone())
                        .status(task_status_to_acp(status))
                        .title(status_message.as_deref().unwrap_or(status)),
                ),
            ))
        }

        AgentEvent::Tool(ToolEvent::Progress { request, progress, total, message }) => {
            Some(map_tool_progress_to_notification(session_id, request, *progress, *total, message.as_deref()))
        }

        AgentEvent::Tool(ToolEvent::DisplayUpdate { request, meta }) => {
            Some(map_display_update_to_notification(session_id, request, meta))
        }

        AgentEvent::Context(
            ContextEvent::Cleared
            | ContextEvent::CompactionStarted { .. }
            | ContextEvent::CompactionEnded { .. }
            | ContextEvent::CompactionResult { .. },
        )
        | AgentEvent::Turn(
            TurnEvent::Started { .. }
            | TurnEvent::Ended { outcome: TurnOutcome::Completed | TurnOutcome::Cancelled | TurnOutcome::Failed { .. } }
            | TurnEvent::RetryScheduled { .. }
            | TurnEvent::LlmCallStarted { .. }
            | TurnEvent::LlmCallEnded { .. }
            | TurnEvent::AutoContinue { .. },
        )
        | AgentEvent::Tool(
            ToolEvent::ExecutionStarted { .. }
            | ToolEvent::DefinitionsUpdated { .. }
            | ToolEvent::SubAgentProgress { .. },
        )
        | AgentEvent::Model(ModelEvent::Switched { .. })
        | AgentEvent::SessionUsage(_) => None,
    }
}

fn json_patch_value(value: serde_json::Value) -> MaybeUndefined<serde_json::Value> {
    match value {
        serde_json::Value::Null => MaybeUndefined::Null,
        value => MaybeUndefined::Value(value),
    }
}

fn task_status_to_acp(status: &str) -> ToolCallStatus {
    match status {
        "working" => ToolCallStatus::InProgress,
        "completed" => ToolCallStatus::Completed,
        "failed" | "cancelled" => ToolCallStatus::Failed,
        _ => ToolCallStatus::Pending,
    }
}

/// Convert internal plan status to ACP protocol status.
fn plan_status_to_acp(status: PlanMetaStatus) -> PlanEntryStatus {
    match status {
        PlanMetaStatus::InProgress => PlanEntryStatus::InProgress,
        PlanMetaStatus::Completed => PlanEntryStatus::Completed,
        PlanMetaStatus::Pending => PlanEntryStatus::Pending,
        // The installed v2 draft has no native cancelled status; custom values must start with `_`.
        PlanMetaStatus::Cancelled => PlanEntryStatus::Other("_aether_cancelled".into()),
    }
}

fn map_chunk_to_notification(
    session_id: SessionId,
    chunk: &str,
    is_complete: bool,
    mode: NotificationMode,
    wrap: fn(ContentChunk) -> SessionUpdate,
    wrap_message: fn(MessageId, Vec<ContentBlock>) -> SessionUpdate,
    message_id: &str,
) -> Option<UpdateSessionNotification> {
    if matches!(mode, NotificationMode::Replay) && !is_complete {
        return None;
    }

    let content = ContentBlock::Text(TextContent::new(chunk));
    let id = MessageId::new(message_id);
    let content_chunk =
        if is_complete { wrap_message(id, vec![content]) } else { wrap(ContentChunk::new(content, id)) };

    Some(acp::UpdateSessionNotification::new(session_id, content_chunk))
}

fn map_tool_call_to_notification(session_id: SessionId, request: &ToolCallRequest) -> UpdateSessionNotification {
    let raw_input = serde_json::from_str(&request.arguments).map_or(MaybeUndefined::Undefined, json_patch_value);
    UpdateSessionNotification::new(
        session_id,
        SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(request.id.clone())
                .title(humanize_tool_name(&request.name))
                .status(acp::ToolCallStatus::InProgress)
                .raw_input(raw_input)
                .meta(aether_tool_name_meta(&request.name)),
        ),
    )
}

fn map_tool_call_update_to_notification(
    session_id: SessionId,
    tool_call_id: &str,
    chunk: &str,
) -> UpdateSessionNotification {
    let update = ToolCallUpdate::new(tool_call_id.to_string())
        .status(ToolCallStatus::InProgress)
        .raw_input(json_patch_value(parse_tool_call_chunk(chunk)));

    UpdateSessionNotification::new(session_id, SessionUpdate::ToolCallUpdate(update))
}

fn map_tool_result_to_notification(
    session_id: SessionId,
    result: &ToolCallResult,
    result_meta: Option<&ToolResultMeta>,
) -> UpdateSessionNotification {
    let mut content = vec![ToolCallContent::Content(Box::new(Content::new(ContentBlock::Text(TextContent::new(
        result.result.clone(),
    )))))];

    if let Some(rm) = result_meta
        && let Some(fd) = &rm.file_diff
        && let Some(diff) = super::diff::map_file_diff(fd)
    {
        content.push(ToolCallContent::Diff(diff));
    }

    let mut update = ToolCallUpdate::new(result.id.clone()).status(ToolCallStatus::Completed).content(content);

    if let Some(rm) = result_meta {
        update = update.title(rm.display.title.clone()).meta(tool_display_meta(&result.name, &rm.display.value));
    }

    UpdateSessionNotification::new(session_id, SessionUpdate::ToolCallUpdate(update))
}

fn map_tool_error_to_notification(session_id: SessionId, error: &ToolCallError) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        session_id,
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(error.id.clone()).status(ToolCallStatus::Failed).content(
            vec![ToolCallContent::Content(Box::new(Content::new(ContentBlock::Text(TextContent::new(
                error.error.clone(),
            )))))],
        )),
    )
}

fn map_context_usage_to_notification(
    session_id: SessionId,
    usage: &llm::ContextUsage,
) -> Option<UpdateSessionNotification> {
    usage.context_limit.map(|context_limit| {
        UpdateSessionNotification::new(
            session_id,
            SessionUpdate::UsageUpdate(UsageUpdate::new(usage.input_tokens.into(), context_limit.into())),
        )
    })
}

fn map_tool_progress_to_notification(
    session_id: SessionId,
    request: &ToolCallRequest,
    progress: f64,
    total: Option<f64>,
    message: Option<&str>,
) -> UpdateSessionNotification {
    tracing::debug!("Tool progress: {message:?}");

    let total_str = total.map_or_else(|| "?".to_string(), |t| t.to_string());
    let progress_text = message
        .map_or_else(|| format!("Progress: {progress}/{total_str}"), |msg| format!("{msg} ({progress}/{total_str})"));

    UpdateSessionNotification::new(
        session_id,
        SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(request.id.clone()).status(ToolCallStatus::InProgress).content(vec![
                ToolCallContent::Content(Box::new(Content::new(ContentBlock::Text(TextContent::new(progress_text))))),
            ]),
        ),
    )
}

fn map_display_update_to_notification(
    session_id: SessionId,
    request: &ToolCallRequest,
    meta: &ToolResultMeta,
) -> UpdateSessionNotification {
    let update = ToolCallUpdate::new(request.id.clone())
        .status(ToolCallStatus::InProgress)
        .title(meta.display.title.clone())
        .meta(tool_display_meta(&request.name, &meta.display.value));

    UpdateSessionNotification::new(session_id, SessionUpdate::ToolCallUpdate(update))
}

fn tool_display_meta(name: &str, value: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = aether_tool_name_meta(name);
    if !value.is_empty() {
        meta.insert("display_value".into(), value.into());
    }
    meta
}

/// Project the full agent event down to the lightweight sub-agent wire type.
fn to_sub_agent_event(event: &AgentEvent) -> SubAgentEvent {
    match event {
        AgentEvent::Tool(ToolEvent::Call { request }) => SubAgentEvent::ToolCall {
            request: SubAgentToolRequest {
                id: request.id.clone(),
                name: request.name.clone(),
                arguments: request.arguments.clone(),
            },
        },
        AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id, chunk }) => SubAgentEvent::ToolCallUpdate {
            update: SubAgentToolCallUpdate { id: tool_call_id.clone(), chunk: chunk.clone() },
        },
        AgentEvent::Tool(ToolEvent::Result { result, result_meta }) => SubAgentEvent::ToolResult {
            result: SubAgentToolResult {
                id: result.id.clone(),
                name: result.name.clone(),
                result_meta: result_meta.clone(),
            },
        },
        AgentEvent::Tool(ToolEvent::Error { error }) => {
            SubAgentEvent::ToolError { error: SubAgentToolError { id: error.id.clone(), name: error.name.clone() } }
        }
        AgentEvent::Turn(TurnEvent::Ended { .. }) => SubAgentEvent::Done,
        _ => SubAgentEvent::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::notifications::SubAgentEvent;
    use aether_core::events::CompactionOutcome;
    use aether_core::events::SubAgentProgressPayload;
    use llm::ContextUsage;
    use llm::ToolCallRequest;

    fn sub_agent_notification(event: AgentEvent) -> SubAgentEvent {
        let event = AgentEvent::Tool(ToolEvent::SubAgentProgress {
            request: ToolCallRequest { id: "parent".into(), name: "spawn".into(), arguments: "{}".into() },
            payload: Box::new(SubAgentProgressPayload { task_id: "task".into(), agent_name: "worker".into(), event }),
        });
        let AgentExtNotification::SubAgentProgress(params) = try_into_agent_notification(&event).unwrap() else {
            panic!("expected subagent progress")
        };
        params.event
    }

    #[test]
    fn task_status_maps_to_acp_lifecycle_status() {
        let request = ToolCallRequest { id: "call-1".into(), name: "tasks__work".into(), arguments: "{}".into() };
        let cases = [
            ("working", ToolCallStatus::InProgress),
            ("input_required", ToolCallStatus::Pending),
            ("completed", ToolCallStatus::Completed),
            ("failed", ToolCallStatus::Failed),
            ("cancelled", ToolCallStatus::Failed),
        ];

        for (status, expected) in cases {
            let event = AgentEvent::Tool(ToolEvent::TaskStatus {
                request: request.clone(),
                task_id: "task-1".into(),
                status: status.into(),
                status_message: None,
            });
            let notification = map_agent_event_to_session_notification(SessionId::new("session"), &event)
                .expect("task status notification");
            let SessionUpdate::ToolCallUpdate(update) = notification.update else {
                panic!("expected tool call update");
            };
            assert_eq!(update.status, MaybeUndefined::Value(expected));
        }
    }

    #[test]
    fn cancelled_task_notification_maps_to_failed_tool_status() {
        let event = AgentEvent::Tool(ToolEvent::TaskCancelled {
            request: ToolCallRequest { id: "call-1".into(), name: "tasks__work".into(), arguments: "{}".into() },
            task_id: "task-1".into(),
        });

        let notification = map_agent_event_to_session_notification(SessionId::new("session"), &event)
            .expect("task cancellation notification");
        let SessionUpdate::ToolCallUpdate(update) = notification.update else {
            panic!("expected tool call update");
        };

        assert_eq!(update.status, MaybeUndefined::Value(ToolCallStatus::Failed));
    }

    #[test]
    fn context_usage_maps_to_native_acp_usage_update() {
        let event = AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: ContextUsage {
                input_tokens: 75_000.into(),
                context_limit: Some(100_000.into()),
                ..ContextUsage::default()
            },
        });

        let notification = map_agent_event_to_session_notification(SessionId::new("session"), &event)
            .expect("context usage notification");
        let SessionUpdate::UsageUpdate(update) = notification.update else {
            panic!("expected usage update");
        };

        assert_eq!(update.used, 75_000);
        assert_eq!(update.size, 100_000);
    }

    #[test]
    fn extension_notifications_report_and_serialize_their_wire_methods() {
        let cases = [
            (
                AgentExtNotification::ContextCompaction(ContextCompactionParams { active: true }),
                "_aether/context_compaction",
            ),
            (AgentExtNotification::ContextCleared(ContextClearedParams::default()), "_aether/context_cleared"),
            (
                AgentExtNotification::SubAgentProgress(Box::new(SubAgentProgressParams {
                    parent_tool_id: "parent".into(),
                    task_id: "task".into(),
                    agent_name: "agent".into(),
                    event: SubAgentEvent::Other,
                })),
                "_aether/sub_agent_progress",
            ),
        ];

        for (notification, expected_method) in cases {
            assert_eq!(notification.method(), expected_method);
            let wire = serde_json::to_value(notification.to_untyped().expect("extension serializes"))
                .expect("untyped message serializes");
            assert_eq!(wire["method"], expected_method);
        }
    }

    #[test]
    fn test_text_includes_message_id() -> Result<(), String> {
        let session_id = SessionId::new("test-session");
        let msg = AgentEvent::Message(MessageEvent::Text {
            message_id: "msg_42".into(),
            chunk: "hello".to_string(),
            is_complete: false,
        });

        let notification = map_agent_event_to_session_notification(session_id, &msg).ok_or("live notification")?;

        let chunk = match notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => chunk,
            other => return Err(format!("Expected AgentEventChunk, got {other:?}")),
        };

        assert_eq!(chunk.message_id, MessageId::new("msg_42"));
        Ok(())
    }

    #[test]
    fn test_thought_includes_message_id() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let msg = AgentEvent::Message(MessageEvent::Thought {
            message_id: "msg_99".into(),
            chunk: "hmm...".to_string(),
            is_complete: false,
        });

        let notification = map_agent_event_to_session_notification(session_id, &msg).ok_or("live notification")?;

        let chunk = match notification.update {
            acp::SessionUpdate::AgentThoughtChunk(chunk) => chunk,
            other => return Err(format!("Expected AgentThoughtChunk, got {other:?}")),
        };
        assert_eq!(chunk.message_id, acp::MessageId::new("msg_99"));
        Ok(())
    }

    #[test]
    fn test_sub_agent_progress_emits_ext_notification() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");

        let payload = SubAgentProgressPayload {
            task_id: "task_1".to_string(),
            agent_name: "sub-agent".to_string(),
            event: AgentEvent::Message(MessageEvent::Text {
                message_id: "msg_1".into(),
                chunk: "Hello".to_string(),
                is_complete: false,
            }),
        };

        let tool_progress = AgentEvent::Tool(ToolEvent::SubAgentProgress {
            request: ToolCallRequest {
                id: "call_123".to_string(),
                name: "plugins__spawn_subagent".to_string(),
                arguments: "{}".to_string(),
            },
            payload: Box::new(payload),
        });

        assert!(map_agent_event_to_session_notification(session_id.clone(), &tool_progress).is_none());

        let agent_notif = try_into_agent_notification(&tool_progress).ok_or("agent notification")?;
        let AgentExtNotification::SubAgentProgress(params) = agent_notif else {
            return Err("expected SubAgentProgress".to_string());
        };
        assert_eq!(params.parent_tool_id, "call_123");
        assert_eq!(params.task_id, "task_1");
        assert_eq!(params.agent_name, "sub-agent");
        assert!(matches!(params.event, SubAgentEvent::Other));
        Ok(())
    }

    #[test]
    fn test_thought_maps_to_agent_thought_chunk_with_message_id() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let thought = AgentEvent::Message(MessageEvent::Thought {
            message_id: "msg_1".into(),
            chunk: "thinking...".to_string(),
            is_complete: false,
        });

        let notification = map_agent_event_to_session_notification(session_id, &thought).ok_or("notification")?;

        let chunk = match notification.update {
            SessionUpdate::AgentThoughtChunk(chunk) => chunk,
            other => return Err(format!("Expected AgentThoughtChunk, got {other:?}")),
        };
        assert_eq!(chunk.message_id, MessageId::new("msg_1"),);
        let text = match chunk.content {
            acp::ContentBlock::Text(text) => text,
            other => return Err(format!("Expected text content, got {other:?}")),
        };
        assert_eq!(text.text, "thinking...");
        Ok(())
    }

    #[test]
    fn test_tool_call_maps_to_tool_call_notification() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let message = AgentEvent::Tool(ToolEvent::Call {
            request: ToolCallRequest {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: "{}".to_string(),
            },
        });

        let notification = map_agent_event_to_session_notification(session_id, &message).ok_or("notification")?;

        let tool_call = match notification.update {
            acp::SessionUpdate::ToolCallUpdate(tool_call) => tool_call,
            other => return Err(format!("Expected ToolCall, got {other:?}")),
        };
        assert_eq!(tool_call.tool_call_id.0.as_ref(), "call_1");
        assert_eq!(tool_call.title, MaybeUndefined::Value("Read file".into()));
        assert_eq!(tool_call.status, MaybeUndefined::Value(acp::ToolCallStatus::InProgress));
        Ok(())
    }

    #[test]
    fn test_tool_call_update_maps_to_tool_call_update_notification() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let message = AgentEvent::Tool(ToolEvent::CallUpdate {
            tool_call_id: "call_1".to_string(),
            chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
        });

        let notification = map_agent_event_to_session_notification(session_id, &message).ok_or("notification")?;

        let update = match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert_eq!(update.tool_call_id.0.as_ref(), "call_1");
        assert_eq!(update.status, MaybeUndefined::Value(acp::ToolCallStatus::InProgress));
        assert_eq!(update.raw_input, MaybeUndefined::Value(serde_json::json!({ "filePath": "Cargo.toml" })));
        Ok(())
    }

    #[test]
    fn test_tool_call_update_has_same_live_and_replay_mapping() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let message = AgentEvent::Tool(ToolEvent::CallUpdate {
            tool_call_id: "call_1".to_string(),
            chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
        });

        let live = map_agent_event_to_session_notification(session_id.clone(), &message).ok_or("live notification")?;
        let replay = map_replayed_agent_event(session_id, &message).ok_or("replay notification")?;

        let (live_update, replay_update) = match (live.update, replay.update) {
            (acp::SessionUpdate::ToolCallUpdate(live), acp::SessionUpdate::ToolCallUpdate(replay)) => (live, replay),
            other => return Err(format!("Expected ToolCallUpdate pair, got {other:?}")),
        };
        assert_eq!(live_update.tool_call_id.0, replay_update.tool_call_id.0);
        assert_eq!(live_update.status, replay_update.status);
        assert_eq!(live_update.raw_input, replay_update.raw_input);
        Ok(())
    }

    #[test]
    fn live_and_replay_map_completed_messages_to_identical_upserts() -> Result<(), String> {
        let cases: Vec<(AgentEvent, &str)> = vec![
            (
                AgentEvent::Message(MessageEvent::Text {
                    message_id: "msg_1".into(),
                    chunk: "done".to_string(),
                    is_complete: true,
                }),
                "done",
            ),
            (
                AgentEvent::Message(MessageEvent::Thought {
                    message_id: "msg_1".into(),
                    chunk: "final reasoning".to_string(),
                    is_complete: true,
                }),
                "final reasoning",
            ),
        ];

        for (message, expected_text) in cases {
            let session_id = acp::SessionId::new("test-session");
            let live =
                map_agent_event_to_session_notification(session_id.clone(), &message).ok_or("live notification")?;
            let notification = map_replayed_agent_event(session_id, &message).ok_or("replay notification")?;
            assert_eq!(live, notification);

            let (id, content) = match notification.update {
                SessionUpdate::AgentMessage(message) => (message.message_id, message.content),
                SessionUpdate::AgentThought(message) => (message.message_id, message.content),
                other => return Err(format!("Expected whole message update, got {other:?}")),
            };
            assert_eq!(id, MessageId::new("msg_1"), "replay preserves original message_id");
            assert_eq!(content, MaybeUndefined::Value(vec![ContentBlock::Text(TextContent::new(expected_text))]));
        }
        Ok(())
    }

    #[test]
    fn test_compaction_lifecycle_maps_to_agent_notifications() -> Result<(), String> {
        let started = AgentEvent::Context(ContextEvent::CompactionStarted { message_count: 12 });
        let started = try_into_agent_notification(&started).ok_or("compaction start notification")?;
        let AgentExtNotification::ContextCompaction(started) = started else {
            return Err("expected ContextCompaction".to_string());
        };
        assert!(started.active);

        for outcome in [
            CompactionOutcome::Completed,
            CompactionOutcome::Failed { error: "failed".to_string() },
            CompactionOutcome::Cancelled,
        ] {
            let ended = AgentEvent::Context(ContextEvent::CompactionEnded { outcome });
            let ended = try_into_agent_notification(&ended).ok_or("compaction end notification")?;
            let AgentExtNotification::ContextCompaction(ended) = ended else {
                return Err("expected ContextCompaction".to_string());
            };
            assert!(!ended.active);
        }

        Ok(())
    }

    #[test]
    fn test_context_cleared_maps_to_agent_notification() -> Result<(), String> {
        let notif = try_into_agent_notification(&AgentEvent::Context(ContextEvent::Cleared))
            .ok_or("context cleared should emit agent notification")?;
        match notif {
            AgentExtNotification::ContextCleared(_) => Ok(()),
            _ => Err("expected ContextCleared".to_string()),
        }
    }

    #[test]
    fn test_tool_progress_with_invalid_json_falls_back_to_simple_message() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");

        // Simulate a tool progress message with invalid JSON
        let tool_progress = AgentEvent::Tool(ToolEvent::Progress {
            request: ToolCallRequest {
                id: "call_456".to_string(),
                name: "some_tool".to_string(),
                arguments: "{}".to_string(),
            },
            progress: 50.0,
            total: None,
            message: Some("not valid json".to_string()),
        });

        let notification = map_agent_event_to_session_notification(session_id.clone(), &tool_progress);

        assert!(notification.is_some());

        // Should still produce a notification with the message as-is
        let notification = notification.ok_or("expected notification")?;
        let SessionUpdate::ToolCallUpdate(update) = notification.update else {
            return Err("Expected ToolCallUpdate".to_string());
        };
        if let MaybeUndefined::Value(content) = &update.content
            && let acp::ToolCallContent::Content(c) = &content[0]
            && let acp::ContentBlock::Text(text) = &c.content
        {
            // Should contain the original message
            assert!(text.text.contains("not valid json"));
        }
        Ok(())
    }

    #[test]
    fn test_tool_call_notification_includes_original_tool_name_meta() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let request = ToolCallRequest {
            id: "call_1".to_string(),
            name: "coding__read_file".to_string(),
            arguments: "{}".to_string(),
        };

        let notification =
            map_agent_event_to_session_notification(session_id, &AgentEvent::Tool(ToolEvent::Call { request }))
                .unwrap();
        let SessionUpdate::ToolCallUpdate(tool_call) = notification.update else {
            return Err("Expected ToolCall".to_string());
        };
        let meta = tool_call.meta.take().ok_or("meta should be present")?;
        assert_eq!(meta.get("aetherToolName").and_then(|value| value.as_str()), Some("coding__read_file"));
        assert_eq!(tool_call.title, MaybeUndefined::Value("Read file".into()));
        Ok(())
    }

    #[test]
    fn test_result_with_result_meta_sets_meta() -> Result<(), String> {
        use mcp_utils::display_meta::ToolDisplayMeta;

        let session_id = acp::SessionId::new("test-session");
        let result = ToolCallResult {
            id: "call_1".to_string(),
            name: "coding__read_file".to_string(),
            arguments: "{}".to_string(),
            result: "file contents".to_string(),
        };
        let rm: ToolResultMeta = ToolDisplayMeta::new("Read file", "Cargo.toml, 156 lines").into();

        let notification = map_agent_event_to_session_notification(
            session_id,
            &AgentEvent::Tool(ToolEvent::Result { result, result_meta: Some(rm) }),
        )
        .unwrap();
        let update = match notification.update {
            SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert_eq!(update.title.value().map(String::as_str), Some("Read file"), "native title should be set");
        let meta = update.meta.take().ok_or("meta should be present")?;
        assert_eq!(
            meta.get("display_value").and_then(|v| v.as_str()),
            Some("Cargo.toml, 156 lines"),
            "display_value should be a flat key in _meta"
        );
        assert!(meta.get("display").is_none(), "old nested display object should not be in _meta");
        Ok(())
    }

    #[test]
    fn test_result_without_result_meta() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let result = ToolCallResult {
            id: "call_1".to_string(),
            name: "external__some_tool".to_string(),
            arguments: "{}".to_string(),
            result: "ok".to_string(),
        };

        let notification = map_agent_event_to_session_notification(
            session_id,
            &AgentEvent::Tool(ToolEvent::Result { result, result_meta: None }),
        )
        .unwrap();
        let update = match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert!(update.title.is_undefined());
        assert!(update.meta.is_undefined());
        Ok(())
    }

    #[test]
    fn test_plan_notification_extracted_from_result_meta() -> Result<(), String> {
        use mcp_utils::display_meta::{PlanMeta, PlanMetaEntry, PlanMetaStatus, ToolDisplayMeta};

        let session_id = acp::SessionId::new("test-session");
        let meta = ToolResultMeta::with_plan(
            ToolDisplayMeta::new("Todo", "Research AI agents"),
            PlanMeta {
                entries: vec![
                    PlanMetaEntry { content: "Research AI agents".to_string(), status: PlanMetaStatus::InProgress },
                    PlanMetaEntry { content: "Write tests".to_string(), status: PlanMetaStatus::Pending },
                ],
            },
        );

        let notification = try_extract_plan_notification(session_id, Some(&meta)).ok_or("should produce plan")?;
        let plan = match notification.update {
            acp::SessionUpdate::PlanUpdate(acp::PlanUpdate { plan: acp::PlanUpdateContent::Items(plan), .. }) => plan,
            other => return Err(format!("Expected Plan, got {other:?}")),
        };
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].content, "Research AI agents");
        assert_eq!(plan.entries[0].status, acp::PlanEntryStatus::InProgress);
        assert_eq!(plan.entries[1].content, "Write tests");
        assert_eq!(plan.entries[1].status, acp::PlanEntryStatus::Pending);
        Ok(())
    }

    #[test]
    fn test_plan_notification_none_when_no_plan_or_no_meta() {
        use mcp_utils::display_meta::ToolDisplayMeta;

        let sid = acp::SessionId::new("test-session");
        let meta: ToolResultMeta = ToolDisplayMeta::new("Read file", "main.rs").into();
        assert!(try_extract_plan_notification(sid.clone(), Some(&meta)).is_none());
        assert!(try_extract_plan_notification(sid, None).is_none());
    }

    #[test]
    fn test_display_update_emits_meta_update() -> Result<(), String> {
        use mcp_utils::display_meta::ToolDisplayMeta;

        let session_id = acp::SessionId::new("test-session");
        let meta = ToolResultMeta::from(ToolDisplayMeta::new("Read file", "main.rs"));

        let request = ToolCallRequest {
            id: "call_789".to_string(),
            name: "coding__read_file".to_string(),
            arguments: "{}".to_string(),
        };

        let event = AgentEvent::Tool(ToolEvent::DisplayUpdate { request, meta });
        let notification = map_agent_event_to_session_notification(session_id, &event)
            .ok_or("display update should produce a notification")?;

        let update = match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert_eq!(&*update.tool_call_id.0, "call_789");
        assert_eq!(update.title.value().map(String::as_str), Some("Read file"), "native title should be set");
        let meta_map = update.meta.take().ok_or("meta should be present")?;
        assert_eq!(
            meta_map.get("display_value").and_then(|v| v.as_str()),
            Some("main.rs"),
            "display_value should be a flat key in _meta"
        );
        assert!(meta_map.get("display").is_none(), "old nested display object should not be in _meta");
        assert_eq!(update.status, MaybeUndefined::Value(acp::ToolCallStatus::InProgress));
        // Should NOT have content (no text progress fallback)
        assert!(update.content.is_undefined());
        Ok(())
    }

    #[test]
    fn test_sub_agent_tool_result_includes_display_fields() {
        use mcp_utils::display_meta::ToolDisplayMeta;

        let event = AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: r#"{"filePath":"Cargo.toml"}"#.to_string(),
                result: "ok".to_string(),
            },
            result_meta: Some(ToolDisplayMeta::new("Read file", "Cargo.toml, 156 lines").into()),
        });

        match sub_agent_notification(event) {
            SubAgentEvent::ToolResult { result } => {
                assert_eq!(result.id, "call_1");
                assert_eq!(result.name, "coding__read_file");
                let result_meta = result.result_meta.expect("result_meta should be present");
                assert_eq!(result_meta.display.title, "Read file");
                assert_eq!(result_meta.display.value, "Cargo.toml, 156 lines");
            }
            other => panic!("Expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn test_sub_agent_tool_call_update_includes_updated_fields() {
        let event = AgentEvent::Tool(ToolEvent::CallUpdate {
            tool_call_id: "call_1".to_string(),
            chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
        });

        match sub_agent_notification(event) {
            SubAgentEvent::ToolCallUpdate { update } => {
                assert_eq!(update.id, "call_1");
                assert_eq!(update.chunk, r#"{"filePath":"Cargo.toml"}"#);
            }
            other => panic!("Expected ToolCallUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_sub_agent_turn_end_maps_to_done() {
        use aether_core::events::TurnOutcome;

        let event = AgentEvent::turn_ended(TurnOutcome::Completed);
        assert!(matches!(sub_agent_notification(event), SubAgentEvent::Done));
    }
}
