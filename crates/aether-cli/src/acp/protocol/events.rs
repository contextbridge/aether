use crate::acp::session::actor::SessionIo;
use acp_utils::notifications::{ContextClearedParams, SessionUsageParams, SubAgentEvent, SubAgentProgressParams};
use aether_core::events::{
    AgentEvent, CompactionOutcome, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome,
    humanize_tool_name,
};
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v2::{
    self as acp, ContentBlock, ContentChunk, MessageId, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionUpdate,
    ToolCallContent, ToolCallStatus, ToolCallUpdate, UsageUpdate,
};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use utils::display_meta::{PlanMetaStatus, ToolResultMeta};

/// Sends updates in delivery order.
pub(crate) fn project_agent_event(msg: &AgentEvent, mode: NotificationMode, io: &SessionIo) {
    if let Some(update) = map_agent_event_to_notification(msg, mode) {
        io.send_update(update);
    }
    if let AgentEvent::Tool(ToolEvent::Result { result_meta, .. }) = msg
        && let Some(update) = try_extract_plan_notification(result_meta.as_ref())
    {
        io.send_update(update);
    }
    if matches!(mode, NotificationMode::Replay) {
        return;
    }
    match msg {
        AgentEvent::Tool(ToolEvent::SubAgentProgress { request, payload }) => {
            if let Some(event) = to_sub_agent_event(&payload.event) {
                io.send(SubAgentProgressParams {
                    session_id: io.session_id().clone(),
                    parent_tool_id: request.id.clone(),
                    task_id: payload.task_id.clone(),
                    agent_name: payload.agent_name.clone(),
                    event,
                });
            }
        }
        AgentEvent::Context(ContextEvent::Cleared) => {
            io.send(ContextClearedParams { session_id: io.session_id().clone() });
        }
        AgentEvent::SessionUsage(usage) => io.send(SessionUsageParams { usage: usage.clone() }),
        _ => {}
    }
}

/// Replace the session's itemized plan with the tool result's plan snapshot.
pub fn try_extract_plan_notification(result_meta: Option<&ToolResultMeta>) -> Option<SessionUpdate> {
    let plan_meta = result_meta?.plan.as_ref()?;
    let entries = plan_meta
        .entries
        .iter()
        .map(|e| PlanEntry::new(e.content.clone(), PlanEntryPriority::Medium, plan_status_to_acp(e.status)))
        .collect();
    Some(SessionUpdate::PlanUpdate(acp::PlanUpdate::new(acp::PlanUpdateContent::items("aether-plan", entries))))
}

#[derive(Clone, Copy)]
pub enum NotificationMode {
    Live,
    Replay,
}

pub fn map_agent_event_to_notification(msg: &AgentEvent, mode: NotificationMode) -> Option<SessionUpdate> {
    match msg {
        AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => map_context_usage_to_notification(usage),

        AgentEvent::Message(MessageEvent::Text { message_id, chunk, is_complete }) => {
            map_message_to_notification(MessageKind::Text, message_id, chunk, *is_complete, mode)
        }

        AgentEvent::Message(MessageEvent::Thought { message_id, chunk, is_complete }) => {
            map_message_to_notification(MessageKind::Thought, message_id, chunk, *is_complete, mode)
        }

        AgentEvent::Tool(ToolEvent::InputStarted { id, name }) => {
            Some(SessionUpdate::ToolCallUpdate(tool_call_upsert(id, name)))
        }

        AgentEvent::Tool(ToolEvent::Call { request }) => Some(map_tool_call_to_notification(request)),

        AgentEvent::Tool(
            ToolEvent::Result { result, result_meta, .. } | ToolEvent::TaskCompleted { result, result_meta, .. },
        ) => Some(map_tool_result_to_notification(result, result_meta.as_ref())),

        AgentEvent::Tool(ToolEvent::Error { error, .. } | ToolEvent::TaskFailed { error, .. }) => {
            Some(map_tool_error_to_notification(error))
        }

        AgentEvent::Tool(ToolEvent::TaskCreated { request, status_message, .. }) => {
            Some(SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(request.id.clone())
                    .status(ToolCallStatus::Pending)
                    .title(status_message.as_deref().unwrap_or("Background task")),
            ))
        }

        AgentEvent::Tool(ToolEvent::TaskCancelled { request, .. }) => Some(SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(request.id.clone())
                .status(ToolCallStatus::Cancelled)
                .content(vec!["The background task was cancelled and will not produce a result.".into()]),
        )),

        AgentEvent::Tool(ToolEvent::TaskStatus { request, status, status_message, .. }) => {
            Some(SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(request.id.clone())
                    .status(task_status_to_acp(status))
                    .title(status_message.as_deref().unwrap_or(status)),
            ))
        }

        AgentEvent::Tool(ToolEvent::Progress { request, progress, total, message }) => {
            Some(map_tool_progress_to_notification(request, *progress, *total, message.as_deref()))
        }

        AgentEvent::Tool(ToolEvent::DisplayUpdate { request, meta }) => {
            Some(map_display_update_to_notification(request, meta))
        }

        AgentEvent::Context(ContextEvent::CompactionStarted { compaction_id, .. }) => {
            Some(SessionUpdate::CompactionUpdate(acp::CompactionUpdate::new(
                compaction_id.as_str(),
                acp::CompactionStatus::InProgress,
            )))
        }
        AgentEvent::Context(ContextEvent::CompactionResult { compaction_id, summary, .. }) => {
            Some(SessionUpdate::CompactionUpdate(
                acp::CompactionUpdate::new(compaction_id.as_str(), acp::CompactionStatus::Completed)
                    .summary(vec![ContentBlock::from(summary.clone())]),
            ))
        }
        AgentEvent::Context(ContextEvent::CompactionEnded { compaction_id, outcome }) => match outcome {
            CompactionOutcome::Completed => None,
            CompactionOutcome::Failed { error } => Some(SessionUpdate::CompactionUpdate(
                acp::CompactionUpdate::new(compaction_id.as_str(), acp::CompactionStatus::Failed).error(error.clone()),
            )),
            CompactionOutcome::Cancelled => Some(SessionUpdate::CompactionUpdate(acp::CompactionUpdate::new(
                compaction_id.as_str(),
                acp::CompactionStatus::Cancelled,
            ))),
        },
        AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { message_id, error } }) => {
            Some(SessionUpdate::AgentMessage(
                acp::AgentMessage::new(MessageId::new(message_id.as_str()))
                    .content(vec![ContentBlock::from(format!("Error: {error}"))]),
            ))
        }
        AgentEvent::Context(ContextEvent::Cleared)
        | AgentEvent::Turn(
            TurnEvent::Started { .. }
            | TurnEvent::UserMessageInserted { .. }
            | TurnEvent::UserMessageDiscarded { .. }
            | TurnEvent::Ended { outcome: TurnOutcome::Completed | TurnOutcome::Cancelled }
            | TurnEvent::RetryScheduled { .. }
            | TurnEvent::LlmCallStarted { .. }
            | TurnEvent::LlmCallEnded { .. }
            | TurnEvent::AutoContinue { .. },
        )
        | AgentEvent::Tool(
            ToolEvent::InputDelta { .. } | ToolEvent::DefinitionsUpdated { .. } | ToolEvent::SubAgentProgress { .. },
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
        "failed" => ToolCallStatus::Failed,
        "cancelled" => ToolCallStatus::Cancelled,
        _ => ToolCallStatus::Pending,
    }
}

/// Convert internal plan status to ACP protocol status.
fn plan_status_to_acp(status: PlanMetaStatus) -> PlanEntryStatus {
    match status {
        PlanMetaStatus::InProgress => PlanEntryStatus::InProgress,
        PlanMetaStatus::Completed => PlanEntryStatus::Completed,
        PlanMetaStatus::Pending => PlanEntryStatus::Pending,
        PlanMetaStatus::Cancelled => PlanEntryStatus::Cancelled,
    }
}

#[derive(Clone, Copy)]
enum MessageKind {
    Text,
    Thought,
}

fn map_message_to_notification(
    kind: MessageKind,
    message_id: &llm::MessageId,
    chunk: &str,
    is_complete: bool,
    mode: NotificationMode,
) -> Option<SessionUpdate> {
    if matches!(mode, NotificationMode::Replay) && !is_complete {
        return None;
    }
    let content = ContentBlock::from(chunk);
    let update = match (kind, is_complete) {
        (MessageKind::Text, false) => {
            SessionUpdate::AgentMessageChunk(ContentChunk::new(content, MessageId::new(message_id.as_str())))
        }
        (MessageKind::Text, true) => SessionUpdate::AgentMessage(
            acp::AgentMessage::new(MessageId::new(message_id.as_str())).content(vec![content]),
        ),
        (MessageKind::Thought, false) => {
            SessionUpdate::AgentThoughtChunk(ContentChunk::new(content, thought_message_id(message_id)))
        }
        (MessageKind::Thought, true) => {
            SessionUpdate::AgentThought(acp::AgentThought::new(thought_message_id(message_id)).content(vec![content]))
        }
    };
    Some(update)
}

fn thought_message_id(message_id: &llm::MessageId) -> MessageId {
    MessageId::new(format!("{message_id}:thought"))
}

fn tool_call_upsert(id: &str, name: &str) -> ToolCallUpdate {
    ToolCallUpdate::new(id.to_string())
        .title(humanize_tool_name(name))
        .status(ToolCallStatus::InProgress)
        .name(name.to_string())
}

fn map_tool_call_to_notification(request: &ToolCallRequest) -> SessionUpdate {
    let raw_input = serde_json::from_str(&request.arguments).map_or(MaybeUndefined::Undefined, json_patch_value);
    SessionUpdate::ToolCallUpdate(tool_call_upsert(&request.id, &request.name).raw_input(raw_input))
}

fn map_tool_result_to_notification(result: &ToolCallResult, result_meta: Option<&ToolResultMeta>) -> SessionUpdate {
    let mut content = vec![ToolCallContent::from(result.result.clone())];

    if let Some(rm) = result_meta
        && let Some(fd) = &rm.file_diff
        && let Some(diff) = super::diff::map_file_diff(fd)
    {
        content.push(diff.into());
    }

    let mut update = ToolCallUpdate::new(result.id.clone()).status(ToolCallStatus::Completed).content(content);

    if let Some(rm) = result_meta {
        update = update.title(rm.display.title.clone()).meta(tool_display_meta(&rm.display.value));
    }

    SessionUpdate::ToolCallUpdate(update)
}

fn map_tool_error_to_notification(error: &ToolCallError) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate(
        ToolCallUpdate::new(error.id.clone()).status(ToolCallStatus::Failed).content(vec![error.error.clone().into()]),
    )
}

fn map_context_usage_to_notification(usage: &llm::ContextUsage) -> Option<SessionUpdate> {
    usage.context_limit.map(|context_limit| {
        SessionUpdate::UsageUpdate(UsageUpdate::new(usage.input_tokens.into(), context_limit.into()))
    })
}

fn map_tool_progress_to_notification(
    request: &ToolCallRequest,
    progress: f64,
    total: Option<f64>,
    message: Option<&str>,
) -> SessionUpdate {
    tracing::debug!("Tool progress: {message:?}");

    let total_str = total.map_or_else(|| "?".to_string(), |t| t.to_string());
    let progress_text = message
        .map_or_else(|| format!("Progress: {progress}/{total_str}"), |msg| format!("{msg} ({progress}/{total_str})"));

    SessionUpdate::ToolCallUpdate(
        ToolCallUpdate::new(request.id.clone()).status(ToolCallStatus::InProgress).content(vec![progress_text.into()]),
    )
}

fn map_display_update_to_notification(request: &ToolCallRequest, meta: &ToolResultMeta) -> SessionUpdate {
    let update = ToolCallUpdate::new(request.id.clone())
        .status(ToolCallStatus::InProgress)
        .title(meta.display.title.clone())
        .name(request.name.clone())
        .meta(tool_display_meta(&meta.display.value));

    SessionUpdate::ToolCallUpdate(update)
}

fn tool_display_meta(value: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    if !value.is_empty() {
        meta.insert("display_value".into(), value.into());
    }
    meta
}

fn to_sub_agent_event(event: &AgentEvent) -> Option<SubAgentEvent> {
    match event {
        AgentEvent::Turn(TurnEvent::Started { .. }) => Some(SubAgentEvent::Started),
        AgentEvent::Turn(TurnEvent::Ended { .. }) => Some(SubAgentEvent::Done),
        _ => match map_agent_event_to_notification(event, NotificationMode::Live)? {
            SessionUpdate::ToolCallUpdate(update) => Some(SubAgentEvent::ToolCallUpdate(Box::new(update))),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::notifications::SubAgentEvent;
    use aether_core::events::{StreamState, SubAgentProgressPayload};
    use agent_client_protocol::schema::v2::TextContent;
    use agent_client_protocol::{Client, JsonRpcNotification};
    use llm::{ContextUsage, ToolCallRequest};
    use serde_json::json;
    use tokio::sync::mpsc::unbounded_channel;
    use utils::display_meta::{PlanMeta, PlanMetaEntry, ToolDisplayMeta};

    fn forwarded<T: JsonRpcNotification + Send + 'static>(events: &[AgentEvent]) -> T {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(
            tokio::task::LocalSet::new().run_until(async {
                let (tx, mut rx) = unbounded_channel();
                let client = Client.v2().on_receive_notification(
                    async move |notification: T, _cx| {
                        tx.send(notification).ok();
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                );
                let agent = agent_client_protocol::Agent.v2().on_receive_request(
                    async |_: acp::InitializeRequest, responder, _cx| {
                        responder.respond(acp_utils::testing::initialize_response())
                    },
                    agent_client_protocol::on_receive_request!(),
                );
                let pair = acp_utils::testing::connect_pair(agent, client).await;
                pair.client.send_request(acp_utils::testing::initialize_request()).block_task().await.unwrap();
                let io = SessionIo::new(Some(pair.agent), "session".into());
                for event in events {
                    project_agent_event(event, NotificationMode::Live, &io);
                }
                rx.recv().await.unwrap()
            }),
        )
    }

    fn sub_agent_progress(event: AgentEvent) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::SubAgentProgress {
            request: ToolCallRequest { id: "parent".into(), name: "spawn".into(), arguments: "{}".into() },
            payload: Box::new(SubAgentProgressPayload { task_id: "task".into(), agent_name: "worker".into(), event }),
        })
    }

    fn sub_agent_notification(event: AgentEvent) -> SubAgentEvent {
        let params: SubAgentProgressParams = forwarded(&[sub_agent_progress(event)]);
        assert_eq!(params.session_id.0.as_ref(), "session");
        params.event
    }

    #[test]
    fn tool_input_start_announces_the_tool_and_deltas_are_not_forwarded() {
        let started = mapped_tool(&AgentEvent::Tool(ToolEvent::InputStarted {
            id: "call_1".into(),
            name: "coding__read_file".into(),
        }));
        assert_eq!(started.title, MaybeUndefined::Value("Read file".into()));
        assert_eq!(started.name.value().map(String::as_str), Some("coding__read_file"));
        assert_eq!(started.status, MaybeUndefined::Value(ToolCallStatus::InProgress));
        assert!(started.raw_input.is_undefined());

        let delta = AgentEvent::Tool(ToolEvent::InputDelta { id: "call_1".into(), chunk: "{".into() });
        assert!(map_agent_event_to_session_notification(&delta).is_none());
    }

    #[test]
    fn sub_agent_tool_calls_forward_the_same_upserts_as_top_level_tools() {
        let request = request("{\"path\":\"a\"}");
        let started = AgentEvent::Tool(ToolEvent::InputStarted { id: request.id.clone(), name: request.name.clone() });
        let display = AgentEvent::Tool(ToolEvent::DisplayUpdate {
            request: request.clone(),
            meta: ToolDisplayMeta::new("Read", "a").into(),
        });
        let result = result_event(Some(ToolDisplayMeta::new("Read file", "Cargo.toml, 156 lines").into()));
        for event in [started, AgentEvent::Tool(ToolEvent::Call { request }), display, result] {
            assert_eq!(
                sub_agent_notification(event.clone()),
                SubAgentEvent::ToolCallUpdate(Box::new(mapped_tool(&event)))
            );
        }
    }

    #[test]
    fn sub_agent_turn_start_and_end_bracket_its_progress() {
        assert_eq!(
            sub_agent_notification(AgentEvent::Turn(TurnEvent::Started { content: vec![] })),
            SubAgentEvent::Started
        );
        assert_eq!(sub_agent_notification(AgentEvent::turn_ended(TurnOutcome::Completed)), SubAgentEvent::Done);
    }

    #[test]
    fn task_status_maps_to_acp_lifecycle_status() {
        let request = ToolCallRequest { id: "call-1".into(), name: "tasks__work".into(), arguments: "{}".into() };
        let cases = [
            ("working", ToolCallStatus::InProgress),
            ("input_required", ToolCallStatus::Pending),
            ("completed", ToolCallStatus::Completed),
            ("failed", ToolCallStatus::Failed),
            ("cancelled", ToolCallStatus::Cancelled),
        ];

        for (status, expected) in cases {
            let event = AgentEvent::Tool(ToolEvent::TaskStatus {
                request: request.clone(),
                task_id: "task-1".into(),
                status: status.into(),
                status_message: None,
            });
            let notification = map_agent_event_to_session_notification(&event).expect("task status notification");
            let SessionUpdate::ToolCallUpdate(update) = notification else {
                panic!("expected tool call update");
            };
            assert_eq!(update.status, MaybeUndefined::Value(expected));
        }
    }

    #[test]
    fn cancelled_task_notification_maps_to_cancelled_tool_status() {
        let event = AgentEvent::Tool(ToolEvent::TaskCancelled {
            request: ToolCallRequest { id: "call-1".into(), name: "tasks__work".into(), arguments: "{}".into() },
            task_id: "task-1".into(),
        });

        let notification = map_agent_event_to_session_notification(&event).expect("task cancellation notification");
        let SessionUpdate::ToolCallUpdate(update) = notification else {
            panic!("expected tool call update");
        };

        assert_eq!(update.status, MaybeUndefined::Value(ToolCallStatus::Cancelled));
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

        let notification = map_agent_event_to_session_notification(&event).expect("context usage notification");
        let SessionUpdate::UsageUpdate(update) = notification else {
            panic!("expected usage update");
        };

        assert_eq!(update.used, 75_000);
        assert_eq!(update.size, 100_000);
    }

    #[test]
    fn sub_agent_progress_skips_events_clients_do_not_track() {
        let untracked = [
            AgentEvent::text("m", "Hel", StreamState::Partial),
            AgentEvent::text("m", "Hello", StreamState::Complete),
            AgentEvent::Tool(ToolEvent::InputDelta { id: "tool".into(), chunk: "{".into() }),
            AgentEvent::Context(ContextEvent::UsageUpdated { usage: ContextUsage::default() }),
        ];
        let events: Vec<_> = untracked
            .into_iter()
            .chain([AgentEvent::turn_ended(TurnOutcome::Completed)])
            .map(sub_agent_progress)
            .collect();
        let params: SubAgentProgressParams = forwarded(&events);

        assert_eq!(
            (params.parent_tool_id.as_str(), params.task_id.as_str(), params.agent_name.as_str()),
            ("parent", "task", "worker")
        );
        assert_eq!(params.event, SubAgentEvent::Done, "untracked events are never sent");
    }

    #[test]
    fn test_tool_call_maps_to_tool_call_notification() -> Result<(), String> {
        let message = AgentEvent::Tool(ToolEvent::Call {
            request: ToolCallRequest {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: "{}".to_string(),
            },
        });

        let notification = map_agent_event_to_session_notification(&message).ok_or("notification")?;

        let tool_call = match notification {
            acp::SessionUpdate::ToolCallUpdate(tool_call) => tool_call,
            other => return Err(format!("Expected ToolCall, got {other:?}")),
        };
        assert_eq!(tool_call.tool_call_id.0.as_ref(), "call_1");
        assert_eq!(tool_call.title, MaybeUndefined::Value("Read file".into()));
        assert_eq!(tool_call.status, MaybeUndefined::Value(acp::ToolCallStatus::InProgress));
        Ok(())
    }

    #[test]
    fn test_context_cleared_maps_to_agent_notification() {
        let cleared: ContextClearedParams = forwarded(&[AgentEvent::Context(ContextEvent::Cleared)]);
        assert_eq!(cleared.session_id.0.as_ref(), "session");
    }

    #[test]
    fn test_tool_progress_with_invalid_json_falls_back_to_simple_message() -> Result<(), String> {
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

        let notification = map_agent_event_to_session_notification(&tool_progress);

        assert!(notification.is_some());

        // Should still produce a notification with the message as-is
        let notification = notification.ok_or("expected notification")?;
        let SessionUpdate::ToolCallUpdate(update) = notification else {
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
    fn test_tool_call_notification_includes_original_tool_name() -> Result<(), String> {
        let request = ToolCallRequest {
            id: "call_1".to_string(),
            name: "coding__read_file".to_string(),
            arguments: "{}".to_string(),
        };

        let notification =
            map_agent_event_to_session_notification(&AgentEvent::Tool(ToolEvent::Call { request })).unwrap();
        let SessionUpdate::ToolCallUpdate(tool_call) = notification else {
            return Err("Expected ToolCall".to_string());
        };
        assert_eq!(tool_call.name.value().map(String::as_str), Some("coding__read_file"));
        assert_eq!(tool_call.title, MaybeUndefined::Value("Read file".into()));
        Ok(())
    }

    #[test]
    fn test_result_with_result_meta_sets_meta() -> Result<(), String> {
        use utils::display_meta::ToolDisplayMeta;

        let result = ToolCallResult {
            id: "call_1".to_string(),
            name: "coding__read_file".to_string(),
            arguments: "{}".to_string(),
            result: "file contents".to_string(),
        };
        let rm: ToolResultMeta = ToolDisplayMeta::new("Read file", "Cargo.toml, 156 lines").into();

        let notification = map_agent_event_to_session_notification(&AgentEvent::Tool(ToolEvent::Result {
            result,
            result_meta: Some(rm),
        }))
        .unwrap();
        let update = match notification {
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
        let result = ToolCallResult {
            id: "call_1".to_string(),
            name: "external__some_tool".to_string(),
            arguments: "{}".to_string(),
            result: "ok".to_string(),
        };

        let notification =
            map_agent_event_to_session_notification(&AgentEvent::Tool(ToolEvent::Result { result, result_meta: None }))
                .unwrap();
        let update = match notification {
            acp::SessionUpdate::ToolCallUpdate(update) => update,
            other => return Err(format!("Expected ToolCallUpdate, got {other:?}")),
        };
        assert!(update.title.is_undefined());
        assert!(update.meta.is_undefined());
        Ok(())
    }

    #[test]
    fn test_plan_notification_none_when_no_plan_or_no_meta() {
        use utils::display_meta::ToolDisplayMeta;

        let meta: ToolResultMeta = ToolDisplayMeta::new("Read file", "main.rs").into();
        assert!(try_extract_plan_notification(Some(&meta)).is_none());
        assert!(try_extract_plan_notification(None).is_none());
    }

    #[test]
    fn test_display_update_emits_meta_update() -> Result<(), String> {
        use utils::display_meta::ToolDisplayMeta;

        let meta = ToolResultMeta::from(ToolDisplayMeta::new("Read file", "main.rs"));

        let request = ToolCallRequest {
            id: "call_789".to_string(),
            name: "coding__read_file".to_string(),
            arguments: "{}".to_string(),
        };

        let event = AgentEvent::Tool(ToolEvent::DisplayUpdate { request, meta });
        let notification =
            map_agent_event_to_session_notification(&event).ok_or("display update should produce a notification")?;

        let update = match notification {
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
    fn tool_upserts_preserve_omitted_null_and_replacement_fields() {
        let mut tool = mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("{\"path\":\"a\"}") }));
        assert_eq!(tool.title.value().map(String::as_str), Some("Read file"));
        assert_eq!(tool.raw_input, MaybeUndefined::Value(json!({"path": "a"})));
        let omitted = mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("invalid json") }));
        assert!(omitted.raw_input.is_undefined());
        assert!(serde_json::to_value(&omitted).unwrap().get("rawInput").is_none());
        tool.apply_update(omitted);
        assert_eq!(tool.raw_input, MaybeUndefined::Value(json!({"path": "a"})));

        let clear = mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("null") }));
        assert!(clear.raw_input.is_null());
        assert_eq!(serde_json::to_value(&clear).unwrap()["rawInput"], json!(null));
        tool.apply_update(clear);
        assert!(tool.raw_input.is_null());
        tool.apply_update(mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("{\"other\":1}") })));
        assert_eq!(tool.raw_input, MaybeUndefined::Value(json!({"other": 1})));
        assert_eq!(tool.name.value().map(String::as_str), Some("coding__read_file"));

        let progress = AgentEvent::Tool(ToolEvent::Progress {
            request: request("{}"),
            progress: 1.0,
            total: Some(2.0),
            message: None,
        });
        tool.apply_update(mapped_tool(&progress));
        let result = mapped_tool(&result_event(None));
        let expected = result.content.clone();
        tool.apply_update(result);
        assert_eq!(tool.content, expected, "result content replaces rather than appends progress");
        assert_eq!(tool.content.value().unwrap().len(), 1);
    }

    #[test]
    fn metadata_replacements_preserve_tool_identity_and_clear_old_display_values() {
        let mut tool = mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("{}") }));
        for value in ["a.rs", ""] {
            tool.apply_update(mapped_tool(&AgentEvent::Tool(ToolEvent::DisplayUpdate {
                request: request("{}"),
                meta: ToolDisplayMeta::new("Read", value).into(),
            })));
            assert_eq!(tool.name.value().map(String::as_str), Some("coding__read_file"));
            assert_eq!(
                tool.meta.value().unwrap().get("display_value").cloned(),
                if value.is_empty() { None } else { Some(json!(value)) }
            );
        }
        tool.apply_update(mapped_tool(&result_event(Some(ToolDisplayMeta::new("Read", "done").into()))));
        assert_eq!(tool.name.value().map(String::as_str), Some("coding__read_file"));
        assert_eq!(tool.meta.value().unwrap()["display_value"], "done");
    }

    #[test]
    fn live_chunks_append_and_replayed_messages_replace_under_stable_ids() {
        for (thought, wire_id) in [(false, "message"), (true, "message:thought")] {
            let mut text = String::new();
            for chunk in ["hello", " world"] {
                let event = message(thought, chunk, false);
                assert!(map_agent_event_to_notification(&event, NotificationMode::Replay).is_none());
                let update = map_agent_event_to_session_notification(&event).unwrap();
                let chunk = match update {
                    SessionUpdate::AgentMessageChunk(c) | SessionUpdate::AgentThoughtChunk(c) => c,
                    other => panic!("expected chunk: {other:?}"),
                };
                assert_eq!(chunk.message_id, MessageId::new(wire_id));
                let ContentBlock::Text(content) = chunk.content else { panic!("expected text") };
                text.push_str(&content.text);
            }
            let complete = message(thought, "hello world", true);
            let live = map_agent_event_to_session_notification(&complete).unwrap();
            let replay = map_agent_event_to_notification(&complete, NotificationMode::Replay).unwrap();
            assert_eq!(live, replay);
            let (id, content) = match replay {
                SessionUpdate::AgentMessage(m) => (m.message_id, m.content),
                SessionUpdate::AgentThought(m) => (m.message_id, m.content),
                other => panic!("expected snapshot: {other:?}"),
            };
            assert_eq!(id, MessageId::new(wire_id));
            assert_eq!(content, MaybeUndefined::Value(vec![ContentBlock::Text(TextContent::new(text))]));
        }
    }

    #[test]
    fn plan_snapshots_replace_entries_with_a_fixed_session_scoped_id() {
        let statuses =
            [PlanMetaStatus::Pending, PlanMetaStatus::InProgress, PlanMetaStatus::Completed, PlanMetaStatus::Cancelled];
        let mut id = None;
        for _ in 0..2 {
            for entries in [
                statuses.iter().map(|status| PlanMetaEntry { content: "task".into(), status: *status }).collect(),
                vec![],
            ] {
                let meta = ToolResultMeta::with_plan(ToolDisplayMeta::new("Todo", ""), PlanMeta { entries });
                let notification = try_extract_plan_notification(Some(&meta)).unwrap();
                let SessionUpdate::PlanUpdate(update) = notification else { panic!("expected plan update") };
                let acp::PlanUpdateContent::Items(plan) = update.plan else { panic!("expected items") };
                assert_eq!(id.get_or_insert(plan.plan_id.clone()), &plan.plan_id);
                let wire = serde_json::to_value(&plan).unwrap();
                if plan.entries.is_empty() {
                    assert_eq!(wire["entries"], json!([]));
                } else {
                    assert_eq!(
                        wire["entries"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|entry| entry["status"].as_str().unwrap())
                            .collect::<Vec<_>>(),
                        ["pending", "in_progress", "completed", "cancelled"]
                    );
                }
            }
        }
    }

    fn request(arguments: &str) -> ToolCallRequest {
        ToolCallRequest { id: "tool".into(), name: "coding__read_file".into(), arguments: arguments.into() }
    }

    fn result_event(result_meta: Option<ToolResultMeta>) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: "tool".into(),
                name: "coding__read_file".into(),
                arguments: "{}".into(),
                result: "done".into(),
            },
            result_meta,
        })
    }

    fn map_agent_event_to_session_notification(event: &AgentEvent) -> Option<SessionUpdate> {
        map_agent_event_to_notification(event, NotificationMode::Live)
    }

    fn mapped_tool(event: &AgentEvent) -> ToolCallUpdate {
        let notification = map_agent_event_to_session_notification(event).unwrap();
        assert_eq!(serde_json::to_value(&notification).unwrap()["sessionUpdate"], "tool_call_update");
        let SessionUpdate::ToolCallUpdate(tool) = notification else { panic!("expected tool upsert") };
        tool
    }

    fn message(thought: bool, chunk: &str, is_complete: bool) -> AgentEvent {
        AgentEvent::Message(if thought {
            MessageEvent::Thought { message_id: "message".into(), chunk: chunk.into(), is_complete }
        } else {
            MessageEvent::Text { message_id: "message".into(), chunk: chunk.into(), is_complete }
        })
    }
}
