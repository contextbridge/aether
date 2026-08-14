use acp_utils::notifications::{
    ContextClearedParams, ContextCompactionParams, ContextUsageParams, SubAgentEvent, SubAgentProgressParams,
    SubAgentToolCallUpdate, SubAgentToolError, SubAgentToolRequest, SubAgentToolResult,
};
use aether_core::events::{
    AgentEvent, ContextEvent, MessageEvent, ModelEvent, SubAgentProgressPayload, ToolEvent, TurnEvent, TurnOutcome,
    aether_tool_name_meta, humanize_tool_name, parse_tool_call_chunk,
};
use agent_client_protocol::schema::v1::{
    self as acp, Content, ContentBlock, ContentChunk, Diff, MessageId, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    SessionId, SessionNotification, SessionUpdate, TextContent, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{JsonRpcMessage, UntypedMessage};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use mcp_utils::display_meta::{PlanMetaStatus, ToolResultMeta};

/// Converts Aether `AgentEvent` to ACP `SessionUpdate`
pub fn map_agent_event_to_session_notification(session_id: SessionId, msg: &AgentEvent) -> Option<SessionNotification> {
    map_agent_event_to_notification(session_id, msg, NotificationMode::Live)
}

/// Typed union of agent-side extension notifications that the actor forwards
/// to the client. Each variant serializes to its own `_aether/*` wire method
/// and is sent via [`ConnectionTo<Client>::send_notification`].
pub enum AgentExtNotification {
    ContextUsage(ContextUsageParams),
    ContextCompaction(ContextCompactionParams),
    ContextCleared(ContextClearedParams),
    SubAgentProgress(SubAgentProgressParams),
}

impl AgentExtNotification {
    pub fn method(&self) -> &str {
        match self {
            Self::ContextUsage(params) => params.method(),
            Self::ContextCompaction(params) => params.method(),
            Self::ContextCleared(params) => params.method(),
            Self::SubAgentProgress(params) => params.method(),
        }
    }

    pub fn to_untyped(&self) -> Result<UntypedMessage, agent_client_protocol::Error> {
        match self {
            Self::ContextUsage(params) => params.to_untyped_message(),
            Self::ContextCompaction(params) => params.to_untyped_message(),
            Self::ContextCleared(params) => params.to_untyped_message(),
            Self::SubAgentProgress(params) => params.to_untyped_message(),
        }
    }
}

pub fn try_into_agent_notification(msg: &AgentEvent) -> Option<AgentExtNotification> {
    match msg {
        AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => {
            Some(AgentExtNotification::ContextUsage(ContextUsageParams { usage: usage.clone() }))
        }
        AgentEvent::Context(ContextEvent::CompactionStarted { .. }) => {
            Some(AgentExtNotification::ContextCompaction(ContextCompactionParams { active: true }))
        }
        AgentEvent::Context(ContextEvent::CompactionEnded { .. }) => {
            Some(AgentExtNotification::ContextCompaction(ContextCompactionParams { active: false }))
        }

        AgentEvent::Tool(ToolEvent::Progress { request, message, .. }) => {
            let msg_str = message.as_ref()?;
            let params = try_parse_sub_agent_progress(msg_str, request)?;
            Some(AgentExtNotification::SubAgentProgress(params))
        }
        AgentEvent::Context(ContextEvent::Cleared) => {
            Some(AgentExtNotification::ContextCleared(ContextClearedParams::default()))
        }
        _ => None,
    }
}

/// If the tool result carries plan metadata, build a `SessionUpdate::Plan` notification.
pub fn try_extract_plan_notification(
    session_id: SessionId,
    result_meta: Option<&ToolResultMeta>,
) -> Option<SessionNotification> {
    let plan_meta = result_meta?.plan.as_ref()?;
    let entries = plan_meta
        .entries
        .iter()
        .map(|e| PlanEntry::new(e.content.clone(), PlanEntryPriority::Medium, plan_status_to_acp(e.status)))
        .collect();
    Some(SessionNotification::new(session_id, SessionUpdate::Plan(acp::Plan::new(entries))))
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
) -> Option<SessionNotification> {
    match msg {
        AgentEvent::Message(MessageEvent::Text { message_id, chunk, is_complete, .. }) => map_chunk_to_notification(
            session_id,
            chunk,
            *is_complete,
            mode,
            SessionUpdate::AgentMessageChunk,
            Some(message_id.as_str()),
        ),

        AgentEvent::Message(MessageEvent::Thought { message_id, chunk, is_complete, .. }) => map_chunk_to_notification(
            session_id,
            chunk,
            *is_complete,
            mode,
            SessionUpdate::AgentThoughtChunk,
            Some(message_id.as_str()),
        ),

        AgentEvent::Tool(ToolEvent::Call { request, .. }) => Some(map_tool_call_to_notification(session_id, request)),

        AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id, chunk, .. }) => {
            Some(map_tool_call_update_to_notification(session_id, tool_call_id, chunk))
        }

        AgentEvent::Tool(
            ToolEvent::Result { result, result_meta, .. } | ToolEvent::TaskCompleted { result, result_meta, .. },
        ) => Some(map_tool_result_to_notification(session_id, result, result_meta.as_ref())),

        AgentEvent::Tool(ToolEvent::Error { error, .. }) => Some(map_tool_error_to_notification(session_id, error)),

        AgentEvent::Tool(ToolEvent::TaskCreated { request, status_message, .. }) => Some(SessionNotification::new(
            session_id,
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new(request.id.clone()),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Pending)
                    .title(status_message.as_deref().unwrap_or("Background task")),
            )),
        )),

        AgentEvent::Tool(ToolEvent::TaskFailed { error, .. }) => {
            Some(map_tool_error_to_notification(session_id, error))
        }

        AgentEvent::Tool(ToolEvent::TaskCancelled { request, .. }) => Some(SessionNotification::new(
            session_id,
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new(request.id.clone()),
                ToolCallUpdateFields::new().status(ToolCallStatus::Failed).content(vec![ToolCallContent::Content(
                    Content::new(ContentBlock::Text(TextContent::new(
                        "The background task was cancelled and will not produce a result.",
                    ))),
                )]),
            )),
        )),

        AgentEvent::Tool(ToolEvent::TaskStatus { request, status, status_message, .. }) => {
            Some(SessionNotification::new(
                session_id,
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    ToolCallId::new(request.id.clone()),
                    ToolCallUpdateFields::new()
                        .status(task_status_to_acp(status))
                        .title(status_message.as_deref().unwrap_or(status)),
                )),
            ))
        }

        AgentEvent::Tool(ToolEvent::Progress { request, progress, total, message }) => {
            map_tool_progress_to_notification(session_id, request, *progress, *total, message.as_ref())
        }

        AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error } }) => {
            Some(acp::SessionNotification::new(
                session_id,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(format!(
                    "[Error] {error}"
                ))))),
            ))
        }

        AgentEvent::Context(
            ContextEvent::UsageUpdated { .. }
            | ContextEvent::Cleared
            | ContextEvent::CompactionStarted { .. }
            | ContextEvent::CompactionEnded { .. }
            | ContextEvent::CompactionResult { .. },
        )
        | AgentEvent::Turn(
            TurnEvent::Started { .. }
            | TurnEvent::Ended { outcome: TurnOutcome::Completed | TurnOutcome::Cancelled }
            | TurnEvent::RetryScheduled { .. }
            | TurnEvent::LlmCallStarted { .. }
            | TurnEvent::LlmCallEnded { .. }
            | TurnEvent::AutoContinue { .. },
        )
        | AgentEvent::Tool(ToolEvent::ExecutionStarted { .. } | ToolEvent::DefinitionsUpdated { .. })
        | AgentEvent::Model(ModelEvent::Switched { .. }) => None,
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
    }
}

fn map_chunk_to_notification(
    session_id: SessionId,
    chunk: &str,
    is_complete: bool,
    mode: NotificationMode,
    wrap: fn(ContentChunk) -> SessionUpdate,
    message_id: Option<&str>,
) -> Option<SessionNotification> {
    match mode {
        // Skip the final completion message to avoid sending duplicate content.
        // The client has already received all the chunks during streaming.
        NotificationMode::Live if is_complete => return None,
        NotificationMode::Replay if !is_complete => return None,
        NotificationMode::Live | NotificationMode::Replay => {}
    }

    let mut content_chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(chunk.to_owned())));
    if let Some(mid) = message_id {
        content_chunk = content_chunk.message_id(MessageId::new(mid));
    }

    Some(acp::SessionNotification::new(session_id, wrap(content_chunk)))
}

fn map_tool_call_to_notification(session_id: SessionId, request: &ToolCallRequest) -> SessionNotification {
    let raw_input = serde_json::from_str(&request.arguments).ok();
    SessionNotification::new(
        session_id,
        SessionUpdate::ToolCall(
            ToolCall::new(ToolCallId::new(request.id.clone()), humanize_tool_name(&request.name))
                .status(acp::ToolCallStatus::InProgress)
                .raw_input(raw_input)
                .meta(aether_tool_name_meta(&request.name)),
        ),
    )
}

fn map_tool_call_update_to_notification(session_id: SessionId, tool_call_id: &str, chunk: &str) -> SessionNotification {
    let fields = ToolCallUpdateFields::new().status(ToolCallStatus::InProgress).raw_input(parse_tool_call_chunk(chunk));

    SessionNotification::new(
        session_id,
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(ToolCallId::new(tool_call_id.to_string()), fields)),
    )
}

fn map_tool_result_to_notification(
    session_id: SessionId,
    result: &ToolCallResult,
    result_meta: Option<&ToolResultMeta>,
) -> SessionNotification {
    let mut content =
        vec![ToolCallContent::Content(Content::new(ContentBlock::Text(TextContent::new(result.result.clone()))))];

    if let Some(rm) = result_meta
        && let Some(fd) = &rm.file_diff
    {
        let mut diff = Diff::new(&fd.path, &fd.new_text);
        if let Some(old) = &fd.old_text {
            diff = diff.old_text(old.clone());
        }
        content.push(ToolCallContent::Diff(diff));
    }

    let mut fields = ToolCallUpdateFields::new().status(ToolCallStatus::Completed).content(content);

    if let Some(rm) = result_meta {
        fields = fields.title(&rm.display.title);
    }

    let mut update = ToolCallUpdate::new(ToolCallId::new(result.id.clone()), fields);

    if let Some(rm) = result_meta
        && !rm.display.value.is_empty()
    {
        let mut meta_map = serde_json::Map::new();
        meta_map.insert("display_value".into(), rm.display.value.clone().into());
        update = update.meta(meta_map);
    }

    SessionNotification::new(session_id, SessionUpdate::ToolCallUpdate(update))
}

fn map_tool_error_to_notification(session_id: SessionId, error: &ToolCallError) -> SessionNotification {
    SessionNotification::new(
        session_id,
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            ToolCallId::new(error.id.clone()),
            ToolCallUpdateFields::new().status(ToolCallStatus::Failed).content(vec![ToolCallContent::Content(
                Content::new(ContentBlock::Text(TextContent::new(error.error.clone()))),
            )]),
        )),
    )
}

fn map_tool_progress_to_notification(
    session_id: SessionId,
    request: &ToolCallRequest,
    progress: f64,
    total: Option<f64>,
    message: Option<&String>,
) -> Option<SessionNotification> {
    tracing::debug!("Tool progress: {message:?}");

    if message.and_then(|msg_str| try_parse_sub_agent_progress(msg_str, request)).is_some() {
        return None;
    }

    if let Some(result_meta) = message.and_then(|m| try_parse_display_meta(m)) {
        let fields = ToolCallUpdateFields::new().status(ToolCallStatus::InProgress).title(&result_meta.display.title);

        let mut update = ToolCallUpdate::new(ToolCallId::new(request.id.clone()), fields);

        if !result_meta.display.value.is_empty() {
            let mut meta_map = serde_json::Map::new();
            meta_map.insert("display_value".into(), result_meta.display.value.into());
            update = update.meta(meta_map);
        }

        return Some(SessionNotification::new(session_id, SessionUpdate::ToolCallUpdate(update)));
    }

    let total_str = total.map_or_else(|| "?".to_string(), |t| t.to_string());
    let progress_text = message
        .map_or_else(|| format!("Progress: {progress}/{total_str}"), |msg| format!("{msg} ({progress}/{total_str})"));

    Some(SessionNotification::new(
        session_id,
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            ToolCallId::new(request.id.clone()),
            ToolCallUpdateFields::new().status(ToolCallStatus::InProgress).content(vec![ToolCallContent::Content(
                Content::new(ContentBlock::Text(TextContent::new(progress_text))),
            )]),
        )),
    ))
}

fn try_parse_display_meta(message: &str) -> Option<ToolResultMeta> {
    serde_json::from_str::<ToolResultMeta>(message).ok()
}

/// Attempt to parse a tool progress message as sub-agent progress.
fn try_parse_sub_agent_progress(message: &str, request: &llm::ToolCallRequest) -> Option<SubAgentProgressParams> {
    let payload: SubAgentProgressPayload = serde_json::from_str(message).ok()?;

    Some(SubAgentProgressParams {
        parent_tool_id: request.id.clone(),
        task_id: payload.task_id,
        agent_name: payload.agent_name,
        event: to_sub_agent_event(&payload.event),
    })
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
    use acp_utils::notifications::{ContextUsage, SubAgentEvent};
    use aether_core::events::CompactionOutcome;
    use llm::ToolCallRequest;

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
            assert_eq!(update.fields.status, Some(expected));
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

        assert_eq!(update.fields.status, Some(ToolCallStatus::Failed));
    }

    #[test]
    fn extension_notifications_report_and_serialize_their_wire_methods() {
        let cases = [
            (
                AgentExtNotification::ContextUsage(ContextUsageParams { usage: ContextUsage::default() }),
                "_aether/context_usage",
            ),
            (
                AgentExtNotification::ContextCompaction(ContextCompactionParams { active: true }),
                "_aether/context_compaction",
            ),
            (AgentExtNotification::ContextCleared(ContextClearedParams::default()), "_aether/context_cleared"),
            (
                AgentExtNotification::SubAgentProgress(SubAgentProgressParams {
                    parent_tool_id: "parent".into(),
                    task_id: "task".into(),
                    agent_name: "agent".into(),
                    event: SubAgentEvent::Other,
                }),
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
            message_id: "msg_42".to_string(),
            chunk: "hello".to_string(),
            is_complete: false,
        });

        let notification =
            map_agent_event_to_notification(session_id, &msg, NotificationMode::Live).ok_or("live notification")?;

        let chunk = match notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => chunk,
            other => return Err(format!("Expected AgentEventChunk, got {other:?}")),
        };

        assert_eq!(chunk.message_id, Some(MessageId::new("msg_42")));
        Ok(())
    }

    #[test]
    fn test_thought_includes_message_id() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let msg = AgentEvent::Message(MessageEvent::Thought {
            message_id: "msg_99".to_string(),
            chunk: "hmm...".to_string(),
            is_complete: false,
        });

        let notification =
            map_agent_event_to_notification(session_id, &msg, NotificationMode::Live).ok_or("live notification")?;

        let chunk = match notification.update {
            acp::SessionUpdate::AgentThoughtChunk(chunk) => chunk,
            other => return Err(format!("Expected AgentThoughtChunk, got {other:?}")),
        };
        assert_eq!(chunk.message_id, Some(acp::MessageId::new("msg_99")));
        Ok(())
    }

    #[test]
    fn test_tool_progress_emits_ext_notification() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");

        let payload = SubAgentProgressPayload {
            task_id: "task_1".to_string(),
            agent_name: "sub-agent".to_string(),
            event: AgentEvent::Message(MessageEvent::Text {
                message_id: "msg_1".to_string(),
                chunk: "Hello".to_string(),
                is_complete: false,
            }),
        };
        let serialized_msg = serde_json::to_string(&payload).unwrap();

        let tool_progress = AgentEvent::Tool(ToolEvent::Progress {
            request: ToolCallRequest {
                id: "call_123".to_string(),
                name: "plugins__spawn_subagent".to_string(),
                arguments: "{}".to_string(),
            },
            progress: 42.0,
            total: Some(100.0),
            message: Some(serialized_msg.clone()),
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
            message_id: "msg_1".to_string(),
            chunk: "thinking...".to_string(),
            is_complete: false,
        });

        let notification = map_agent_event_to_session_notification(session_id, &thought).ok_or("notification")?;

        let chunk = match notification.update {
            SessionUpdate::AgentThoughtChunk(chunk) => chunk,
            other => return Err(format!("Expected AgentThoughtChunk, got {other:?}")),
        };
        assert_eq!(chunk.message_id, Some(MessageId::new("msg_1")),);
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
            acp::SessionUpdate::ToolCall(tool_call) => tool_call,
            other => return Err(format!("Expected ToolCall, got {other:?}")),
        };
        assert_eq!(tool_call.tool_call_id.0.as_ref(), "call_1");
        assert_eq!(tool_call.title, "Read file");
        assert_eq!(tool_call.status, acp::ToolCallStatus::InProgress);
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
        assert_eq!(update.fields.status, Some(acp::ToolCallStatus::InProgress));
        assert_eq!(update.fields.raw_input, Some(serde_json::json!({ "filePath": "Cargo.toml" })));
        Ok(())
    }

    #[test]
    fn test_tool_call_update_has_same_live_and_replay_mapping() -> Result<(), String> {
        let session_id = acp::SessionId::new("test-session");
        let message = AgentEvent::Tool(ToolEvent::CallUpdate {
            tool_call_id: "call_1".to_string(),
            chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
        });

        let live = map_agent_event_to_notification(session_id.clone(), &message, NotificationMode::Live)
            .ok_or("live notification")?;
        let replay = map_agent_event_to_notification(session_id, &message, NotificationMode::Replay)
            .ok_or("replay notification")?;

        let (live_update, replay_update) = match (live.update, replay.update) {
            (acp::SessionUpdate::ToolCallUpdate(live), acp::SessionUpdate::ToolCallUpdate(replay)) => (live, replay),
            other => return Err(format!("Expected ToolCallUpdate pair, got {other:?}")),
        };
        assert_eq!(live_update.tool_call_id.0, replay_update.tool_call_id.0);
        assert_eq!(live_update.fields.status, replay_update.fields.status);
        assert_eq!(live_update.fields.raw_input, replay_update.fields.raw_input);
        Ok(())
    }

    #[test]
    fn test_live_mapping_skips_completed_chunks_but_replay_keeps_them() -> Result<(), String> {
        let cases: Vec<(AgentEvent, &str)> = vec![
            (
                AgentEvent::Message(MessageEvent::Text {
                    message_id: "msg_1".to_string(),
                    chunk: "done".to_string(),
                    is_complete: true,
                }),
                "done",
            ),
            (
                AgentEvent::Message(MessageEvent::Thought {
                    message_id: "msg_1".to_string(),
                    chunk: "final reasoning".to_string(),
                    is_complete: true,
                }),
                "final reasoning",
            ),
        ];

        for (message, expected_text) in cases {
            let session_id = acp::SessionId::new("test-session");
            assert!(
                map_agent_event_to_notification(session_id.clone(), &message, NotificationMode::Live).is_none(),
                "live mode should skip completed chunk"
            );

            let notification = map_agent_event_to_notification(session_id, &message, NotificationMode::Replay)
                .ok_or("replay notification")?;

            let chunk = match notification.update {
                SessionUpdate::AgentMessageChunk(chunk) | SessionUpdate::AgentThoughtChunk(chunk) => chunk,
                other => return Err(format!("Expected chunk update, got {other:?}")),
            };
            assert_eq!(
                chunk.message_id,
                Some(acp::MessageId::new("msg_1")),
                "replay should preserve original message_id"
            );
            let text = match chunk.content {
                acp::ContentBlock::Text(text) => text,
                other => return Err(format!("Expected text content, got {other:?}")),
            };
            assert_eq!(text.text, expected_text);
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
        if let Some(content) = &update.fields.content
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

        let notification = map_tool_call_to_notification(session_id, &request);
        let SessionUpdate::ToolCall(tool_call) = notification.update else {
            return Err("Expected ToolCall".to_string());
        };
        let meta = tool_call.meta.ok_or("meta should be present")?;
        assert_eq!(meta.get("aetherToolName").and_then(|value| value.as_str()), Some("coding__read_file"));
        assert_eq!(tool_call.title, "Read file");
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

        let notification = map_tool_result_to_notification(session_id, &result, Some(&rm));
        let update = match notification.update {
            SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert_eq!(update.fields.title.as_deref(), Some("Read file"), "native title should be set");
        let meta = update.meta.ok_or("meta should be present")?;
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

        let notification = map_tool_result_to_notification(session_id, &result, None);
        let update = match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert!(update.fields.title.is_none());
        assert!(update.meta.is_none());
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
            acp::SessionUpdate::Plan(plan) => plan,
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
    fn test_tool_progress_with_display_meta_emits_meta_update() -> Result<(), String> {
        use mcp_utils::display_meta::ToolDisplayMeta;

        let session_id = acp::SessionId::new("test-session");
        let meta = ToolResultMeta::from(ToolDisplayMeta::new("Read file", "main.rs"));
        let serialized = serde_json::to_string(&meta).unwrap();

        let request = ToolCallRequest {
            id: "call_789".to_string(),
            name: "coding__read_file".to_string(),
            arguments: "{}".to_string(),
        };

        let notification = map_tool_progress_to_notification(session_id, &request, 0.0, None, Some(&serialized))
            .ok_or("should produce notification")?;

        let update = match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert_eq!(&*update.tool_call_id.0, "call_789");
        assert_eq!(update.fields.title.as_deref(), Some("Read file"), "native title should be set");
        let meta_map = update.meta.ok_or("meta should be present")?;
        assert_eq!(
            meta_map.get("display_value").and_then(|v| v.as_str()),
            Some("main.rs"),
            "display_value should be a flat key in _meta"
        );
        assert!(meta_map.get("display").is_none(), "old nested display object should not be in _meta");
        assert_eq!(update.fields.status, Some(acp::ToolCallStatus::InProgress));
        // Should NOT have content (no text progress fallback)
        assert!(update.fields.content.is_none());
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

        match to_sub_agent_event(&event) {
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

        match to_sub_agent_event(&event) {
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
        assert!(matches!(to_sub_agent_event(&event), SubAgentEvent::Done));
    }
}
