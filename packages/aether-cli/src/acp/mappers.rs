use acp_utils::notifications::{ContextClearedParams, ContextUsageParams, SubAgentProgressParams};
use acp_utils::server::AcpServerError;
use aether_core::events::{AgentMessage, SubAgentProgressPayload};
use agent_client_protocol::schema::{
    self as acp, Content, ContentBlock, ContentChunk, Diff, HttpHeader, McpServer, PlanEntry, PlanEntryPriority,
    PlanEntryStatus, SessionId, SessionNotification, SessionUpdate, StopReason, TextContent, ToolCall, ToolCallContent,
    ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use mcp_utils::client::{McpServer as RuntimeMcpServer, McpTransport};
use mcp_utils::display_meta::{PlanMetaStatus, ToolResultMeta};
use rmcp::model::Prompt as McpPrompt;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

use aether_core::context::ext::{SessionEvent, UserEvent};

/// Converts an MCP Prompt to an ACP `AvailableCommand`
///
/// Strips the MCP namespace from the prompt name (e.g., "`coding__web`" -> "web")
/// and creates a slash command that clients can invoke.
pub fn map_mcp_prompt_to_available_command(prompt: &McpPrompt) -> acp::AvailableCommand {
    // Extract the base command name by removing the namespace prefix
    let command_name = prompt.name.split("__").last().unwrap_or(prompt.name.as_ref()).to_string();

    // Extract the input hint from the unified prompt format's ARGUMENTS parameter,
    // falling back to a generic hint.
    let hint = prompt
        .arguments
        .as_ref()
        .and_then(|args| args.iter().find(|a| a.name.as_str() == "ARGUMENTS").and_then(|a| a.description.as_deref()))
        .unwrap_or("optional arguments");
    let input = Some(acp::AvailableCommandInput::Unstructured(acp::UnstructuredCommandInput::new(hint)));

    let description = prompt.description.clone().unwrap_or_else(|| "No description available".to_string());

    acp::AvailableCommand::new(command_name, description).input(input)
}

/// Maps ACP MCP server definitions to internal MCP servers, skipping unsupported transports.
pub fn map_acp_mcp_servers(servers: Vec<McpServer>) -> Vec<RuntimeMcpServer> {
    servers
        .into_iter()
        .filter_map(|s| {
            try_map_mcp_server(s).or_else(|| {
                tracing::warn!("Unsupported ACP MCP server transport, skipping");
                None
            })
        })
        .collect()
}

fn try_map_mcp_server(server: McpServer) -> Option<RuntimeMcpServer> {
    use McpServer::{Http, Sse, Stdio};
    match server {
        Stdio(stdio) => Some(RuntimeMcpServer::new(
            stdio.name,
            McpTransport::Stdio {
                command: stdio.command.to_string_lossy().into_owned(),
                args: stdio.args,
                env: stdio.env.into_iter().map(|e| (e.name, e.value)).collect(),
            },
            false,
        )),

        Http(http) => Some(RuntimeMcpServer::new(
            http.name,
            McpTransport::Http { config: http_config(http.url, &http.headers) },
            false,
        )),

        Sse(sse) => Some(RuntimeMcpServer::new(
            sse.name,
            McpTransport::Http { config: http_config(sse.url, &sse.headers) },
            false,
        )),

        _ => None,
    }
}

fn http_config(url: String, headers: &[HttpHeader]) -> StreamableHttpClientTransportConfig {
    let auth_header = headers.iter().find(|h| h.name.eq_ignore_ascii_case("authorization")).map(|h| h.value.clone());

    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    if let Some(auth) = auth_header {
        // rmcp's `auth_header` wants the bare token; it adds the `Bearer ` prefix itself.
        let token = auth
            .split_once(' ')
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map_or(auth.as_str(), |(_, rest)| rest);
        config = config.auth_header(token.to_string());
    }
    config
}

/// Converts Aether `AgentMessage` to ACP `SessionUpdate`
pub fn map_agent_message_to_session_notification(
    session_id: SessionId,
    msg: &AgentMessage,
) -> Option<SessionNotification> {
    map_agent_message_to_notification(session_id, msg, NotificationMode::Live)
}

#[derive(Clone, Copy)]
enum NotificationMode {
    Live,
    Replay,
}

fn map_agent_message_to_notification(
    session_id: SessionId,
    msg: &AgentMessage,
    mode: NotificationMode,
) -> Option<SessionNotification> {
    match msg {
        AgentMessage::Text { chunk, is_complete, .. } => {
            map_chunk_to_notification(session_id, chunk, *is_complete, mode, SessionUpdate::AgentMessageChunk)
        }

        AgentMessage::Thought { chunk, is_complete, .. } => {
            map_chunk_to_notification(session_id, chunk, *is_complete, mode, SessionUpdate::AgentThoughtChunk)
        }

        AgentMessage::ToolCall { request, .. } => Some(map_tool_call_to_notification(session_id, request)),

        AgentMessage::ToolCallUpdate { tool_call_id, chunk, .. } => {
            Some(map_tool_call_update_to_notification(session_id, tool_call_id, chunk))
        }

        AgentMessage::ToolResult { result, result_meta, .. } => {
            Some(map_tool_result_to_notification(session_id, result, result_meta.as_ref()))
        }

        AgentMessage::ToolError { error, .. } => Some(map_tool_error_to_notification(session_id, error)),

        AgentMessage::ToolProgress { request, progress, total, message } => {
            map_tool_progress_to_notification(session_id, request, *progress, *total, message.as_ref())
        }

        AgentMessage::Error { message } => Some(acp::SessionNotification::new(
            session_id,
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(format!(
                "[Error] {message}"
            ))))),
        )),

        AgentMessage::ContextUsageUpdate { .. }
        | AgentMessage::ContextCleared
        | AgentMessage::Cancelled { .. }
        | AgentMessage::Done
        | AgentMessage::ContextCompactionStarted { .. }
        | AgentMessage::ContextCompactionResult { .. }
        | AgentMessage::AutoContinue { .. }
        | AgentMessage::Retrying { .. }
        | AgentMessage::ModelSwitched { .. } => None,
    }
}

/// Typed union of agent-side extension notifications that the relay forwards
/// to the client. Each variant serializes to its own `_aether/*` wire method
/// and is sent via [`ConnectionTo<Client>::send_notification`].
pub enum AgentExtNotification {
    ContextUsage(ContextUsageParams),
    ContextCleared(ContextClearedParams),
    SubAgentProgress(SubAgentProgressParams),
}

pub fn try_into_agent_notification(msg: &AgentMessage) -> Option<AgentExtNotification> {
    match msg {
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
        } => Some(AgentExtNotification::ContextUsage(ContextUsageParams {
            usage_ratio: *usage_ratio,
            context_limit: *context_limit,
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
            cache_read_tokens: *cache_read_tokens,
            cache_creation_tokens: *cache_creation_tokens,
            reasoning_tokens: *reasoning_tokens,
            total_input_tokens: *total_input_tokens,
            total_output_tokens: *total_output_tokens,
            total_cache_read_tokens: *total_cache_read_tokens,
            total_cache_creation_tokens: *total_cache_creation_tokens,
            total_reasoning_tokens: *total_reasoning_tokens,
        })),
        AgentMessage::ToolProgress { request, message, .. } => {
            let msg_str = message.as_ref()?;
            let params = try_parse_sub_agent_progress(msg_str, request)?;
            Some(AgentExtNotification::SubAgentProgress(params))
        }
        AgentMessage::ContextCleared => Some(AgentExtNotification::ContextCleared(ContextClearedParams::default())),
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

/// Convert internal plan status to ACP protocol status.
fn plan_status_to_acp(status: PlanMetaStatus) -> PlanEntryStatus {
    match status {
        PlanMetaStatus::InProgress => PlanEntryStatus::InProgress,
        PlanMetaStatus::Completed => PlanEntryStatus::Completed,
        PlanMetaStatus::Pending => PlanEntryStatus::Pending,
    }
}

/// Determines the stop reason from the final agent message
pub fn map_agent_message_to_stop_reason(msg: &AgentMessage) -> acp::StopReason {
    match msg {
        AgentMessage::Cancelled { .. } => StopReason::Cancelled,
        _ => StopReason::EndTurn,
    }
}

fn map_chunk_to_notification(
    session_id: SessionId,
    chunk: &str,
    is_complete: bool,
    mode: NotificationMode,
    wrap: fn(ContentChunk) -> SessionUpdate,
) -> Option<SessionNotification> {
    match mode {
        // Skip the final completion message to avoid sending duplicate content.
        // The client has already received all the chunks during streaming.
        NotificationMode::Live if is_complete => return None,
        NotificationMode::Replay if !is_complete => return None,
        NotificationMode::Live | NotificationMode::Replay => {}
    }

    Some(acp::SessionNotification::new(
        session_id,
        wrap(ContentChunk::new(ContentBlock::Text(TextContent::new(chunk.to_owned())))),
    ))
}

fn map_tool_call_to_notification(session_id: SessionId, request: &ToolCallRequest) -> SessionNotification {
    let raw_input = serde_json::from_str(&request.arguments).ok();
    SessionNotification::new(
        session_id,
        SessionUpdate::ToolCall(
            ToolCall::new(ToolCallId::new(request.id.clone()), humanize_tool_name(&request.name))
                .status(acp::ToolCallStatus::InProgress)
                .raw_input(raw_input),
        ),
    )
}

fn parse_tool_call_chunk(chunk: &str) -> serde_json::Value {
    serde_json::from_str(chunk).unwrap_or_else(|_| serde_json::Value::String(chunk.to_string()))
}

fn map_tool_call_update_to_notification(session_id: SessionId, tool_call_id: &str, chunk: &str) -> SessionNotification {
    let fields = ToolCallUpdateFields::new().status(ToolCallStatus::InProgress).raw_input(parse_tool_call_chunk(chunk));

    SessionNotification::new(
        session_id,
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(ToolCallId::new(tool_call_id.to_string()), fields)),
    )
}

/// Produces the initial human-readable title for a tool call (e.g., "Read file").
/// This is sent when the tool call starts.
fn humanize_tool_name(name: &str) -> String {
    let base = name.split("__").last().unwrap_or(name);
    let mut result = base.replace('_', " ");
    if let Some(first) = result.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    result
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

/// Replay session events to the client as ACP notifications.
pub async fn replay_to_client(events: &[SessionEvent], connection: &ConnectionTo<Client>, session_id: &SessionId) {
    for notif in replay_events_to_notifications(events, session_id) {
        if let Err(e) = connection.send_notification(notif).map_err(|e| AcpServerError::protocol("session/update", e)) {
            tracing::error!("Failed to send replay notification: {e:?}");
        }
    }
}

pub fn replay_events_to_notifications(events: &[SessionEvent], session_id: &SessionId) -> Vec<SessionNotification> {
    let mut out = Vec::new();
    for event in events {
        match event {
            SessionEvent::User(UserEvent::Message { content }) => {
                for block in content {
                    out.push(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::UserMessageChunk(ContentChunk::new(map_user_content_block(block))),
                    ));
                }
            }
            SessionEvent::Agent(message) => {
                out.extend(map_agent_message_to_notification(session_id.clone(), message, NotificationMode::Replay));
            }
            SessionEvent::User(_) => {}
        }
    }
    out
}

fn map_user_content_block(block: &llm::ContentBlock) -> ContentBlock {
    match block {
        llm::ContentBlock::Text { text } => ContentBlock::Text(TextContent::new(text.clone())),
        llm::ContentBlock::Image { data, mime_type } => {
            ContentBlock::Image(acp::ImageContent::new(data.clone(), mime_type.clone()))
        }
        llm::ContentBlock::Audio { data, mime_type } => {
            ContentBlock::Audio(acp::AudioContent::new(data.clone(), mime_type.clone()))
        }
    }
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
        event: (&payload.event).into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::notifications::SubAgentEvent;
    use llm::ToolCallRequest;

    #[test]
    fn test_tool_progress_with_sub_agent_payload_emits_ext_notification() {
        let session_id = acp::SessionId::new("test-session");

        let payload = SubAgentProgressPayload {
            task_id: "task_1".to_string(),
            agent_name: "sub-agent".to_string(),
            event: AgentMessage::Text {
                message_id: "msg_1".to_string(),
                chunk: "Hello".to_string(),
                is_complete: false,
                model_name: "TestModel".to_string(),
            },
        };
        let serialized_msg = serde_json::to_string(&payload).unwrap();

        let tool_progress = AgentMessage::ToolProgress {
            request: ToolCallRequest {
                id: "call_123".to_string(),
                name: "plugins__spawn_subagent".to_string(),
                arguments: "{}".to_string(),
            },
            progress: 42.0,
            total: Some(100.0),
            message: Some(serialized_msg.clone()),
        };

        let notification = map_agent_message_to_session_notification(session_id.clone(), &tool_progress);

        assert!(notification.is_none());

        let agent_notif = try_into_agent_notification(&tool_progress).expect("agent notification");
        match agent_notif {
            AgentExtNotification::SubAgentProgress(params) => {
                assert_eq!(params.parent_tool_id, "call_123");
                assert_eq!(params.task_id, "task_1");
                assert_eq!(params.agent_name, "sub-agent");
                assert!(matches!(params.event, SubAgentEvent::Other));
            }
            _ => panic!("expected SubAgentProgress"),
        }
    }

    #[test]
    fn replay_emits_user_media_chunks_in_order() {
        let session_id = acp::SessionId::new("test-session");
        let events = vec![SessionEvent::User(UserEvent::Message {
            content: vec![
                llm::ContentBlock::text("hello"),
                llm::ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() },
                llm::ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() },
            ],
        })];

        let notifications = replay_events_to_notifications(&events, &session_id);
        let updates: Vec<_> = notifications.into_iter().map(|n| n.update).collect();
        assert!(matches!(
            &updates[0],
            acp::SessionUpdate::UserMessageChunk(chunk)
                if matches!(&chunk.content, acp::ContentBlock::Text(text) if text.text == "hello")
        ));
        assert!(matches!(
            &updates[1],
            acp::SessionUpdate::UserMessageChunk(chunk)
                if matches!(&chunk.content, acp::ContentBlock::Image(_))
        ));
        assert!(matches!(
            &updates[2],
            acp::SessionUpdate::UserMessageChunk(chunk)
                if matches!(&chunk.content, acp::ContentBlock::Audio(_))
        ));
    }

    #[test]
    fn test_thought_maps_to_agent_thought_chunk() {
        let session_id = acp::SessionId::new("test-session");
        let thought = AgentMessage::Thought {
            message_id: "msg_1".to_string(),
            chunk: "thinking...".to_string(),
            is_complete: false,
            model_name: "TestModel".to_string(),
        };

        let notification = map_agent_message_to_session_notification(session_id, &thought).expect("notification");

        match notification.update {
            acp::SessionUpdate::AgentThoughtChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(text) => assert_eq!(text.text, "thinking..."),
                other => panic!("Expected text content, got {other:?}"),
            },
            other => panic!("Expected AgentThoughtChunk, got {other:?}"),
        }
    }

    #[test]
    fn test_tool_call_maps_to_tool_call_notification() {
        let session_id = acp::SessionId::new("test-session");
        let message = AgentMessage::ToolCall {
            request: ToolCallRequest {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: "{}".to_string(),
            },
            model_name: "TestModel".to_string(),
        };

        let notification = map_agent_message_to_session_notification(session_id, &message).expect("notification");

        match notification.update {
            acp::SessionUpdate::ToolCall(tool_call) => {
                assert_eq!(tool_call.tool_call_id.0.as_ref(), "call_1");
                assert_eq!(tool_call.title, "Read file");
                assert_eq!(tool_call.status, acp::ToolCallStatus::InProgress);
            }
            other => panic!("Expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn test_tool_call_update_maps_to_tool_call_update_notification() {
        let session_id = acp::SessionId::new("test-session");
        let message = AgentMessage::ToolCallUpdate {
            tool_call_id: "call_1".to_string(),
            chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
            model_name: "TestModel".to_string(),
        };

        let notification = map_agent_message_to_session_notification(session_id, &message).expect("notification");

        match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => {
                assert_eq!(update.tool_call_id.0.as_ref(), "call_1");
                assert_eq!(update.fields.status, Some(acp::ToolCallStatus::InProgress));
                assert_eq!(update.fields.raw_input, Some(serde_json::json!({ "filePath": "Cargo.toml" })));
            }
            other => panic!("Expected ToolCallUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_tool_call_update_has_same_live_and_replay_mapping() {
        let session_id = acp::SessionId::new("test-session");
        let message = AgentMessage::ToolCallUpdate {
            tool_call_id: "call_1".to_string(),
            chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
            model_name: "TestModel".to_string(),
        };

        let live = map_agent_message_to_notification(session_id.clone(), &message, NotificationMode::Live)
            .expect("live notification");
        let replay = map_agent_message_to_notification(session_id, &message, NotificationMode::Replay)
            .expect("replay notification");

        match (live.update, replay.update) {
            (acp::SessionUpdate::ToolCallUpdate(live), acp::SessionUpdate::ToolCallUpdate(replay)) => {
                assert_eq!(live.tool_call_id.0, replay.tool_call_id.0);
                assert_eq!(live.fields.status, replay.fields.status);
                assert_eq!(live.fields.raw_input, replay.fields.raw_input);
            }
            other => panic!("Expected ToolCallUpdate pair, got {other:?}"),
        }
    }

    #[test]
    fn test_live_mapping_skips_completed_chunks_but_replay_keeps_them() {
        let cases: Vec<(AgentMessage, &str)> = vec![
            (
                AgentMessage::Text {
                    message_id: "msg_1".to_string(),
                    chunk: "done".to_string(),
                    is_complete: true,
                    model_name: "TestModel".to_string(),
                },
                "done",
            ),
            (
                AgentMessage::Thought {
                    message_id: "msg_1".to_string(),
                    chunk: "final reasoning".to_string(),
                    is_complete: true,
                    model_name: "TestModel".to_string(),
                },
                "final reasoning",
            ),
        ];

        for (message, expected_text) in cases {
            let session_id = acp::SessionId::new("test-session");
            assert!(
                map_agent_message_to_notification(session_id.clone(), &message, NotificationMode::Live).is_none(),
                "live mode should skip completed chunk"
            );

            let notification = map_agent_message_to_notification(session_id, &message, NotificationMode::Replay)
                .expect("replay notification");

            match notification.update {
                acp::SessionUpdate::AgentMessageChunk(chunk) | acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                    match chunk.content {
                        acp::ContentBlock::Text(text) => assert_eq!(text.text, expected_text),
                        other => panic!("Expected text content, got {other:?}"),
                    }
                }
                other => panic!("Expected chunk update, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_context_cleared_maps_to_agent_notification() {
        let notif = try_into_agent_notification(&AgentMessage::ContextCleared)
            .expect("context cleared should emit agent notification");
        assert!(matches!(notif, AgentExtNotification::ContextCleared(_)));
    }

    #[test]
    fn test_tool_progress_with_invalid_json_falls_back_to_simple_message() {
        let session_id = acp::SessionId::new("test-session");

        // Simulate a tool progress message with invalid JSON
        let tool_progress = AgentMessage::ToolProgress {
            request: ToolCallRequest {
                id: "call_456".to_string(),
                name: "some_tool".to_string(),
                arguments: "{}".to_string(),
            },
            progress: 50.0,
            total: None,
            message: Some("not valid json".to_string()),
        };

        let notification = map_agent_message_to_session_notification(session_id.clone(), &tool_progress);

        assert!(notification.is_some());

        // Should still produce a notification with the message as-is
        let notification = notification.unwrap();
        match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => {
                if let Some(content) = &update.fields.content
                    && let acp::ToolCallContent::Content(c) = &content[0]
                    && let acp::ContentBlock::Text(text) = &c.content
                {
                    // Should contain the original message
                    assert!(text.text.contains("not valid json"));
                }
            }
            _ => panic!("Expected ToolCallUpdate"),
        }
    }

    #[test]
    fn test_map_acp_stdio_server() {
        let server = acp::McpServer::Stdio(
            acp::McpServerStdio::new("my-server", "/usr/bin/server")
                .args(vec!["--port".into(), "8080".into()])
                .env(vec![acp::EnvVariable::new("FOO", "bar")]),
        );

        let configs = map_acp_mcp_servers(vec![server]);
        assert_eq!(configs.len(), 1);

        match &configs[0].transport {
            McpTransport::Stdio { command, args, env } => {
                assert_eq!(configs[0].name, "my-server");
                assert_eq!(command, "/usr/bin/server");
                assert_eq!(args, &["--port", "8080"]);
                assert_eq!(env.get("FOO").unwrap(), "bar");
            }
            other => panic!("Expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn test_map_acp_http_server() {
        let server = acp::McpServer::Http(
            acp::McpServerHttp::new("http-server", "https://example.com/mcp")
                .headers(vec![acp::HttpHeader::new("Authorization", "Bearer token123")]),
        );

        let configs = map_acp_mcp_servers(vec![server]);
        assert_eq!(configs.len(), 1);

        match &configs[0].transport {
            McpTransport::Http { config } => {
                assert_eq!(configs[0].name, "http-server");
                assert_eq!(config.uri.as_ref(), "https://example.com/mcp");
                assert_eq!(config.auth_header.as_deref(), Some("token123"));
            }
            other => panic!("Expected Http, got {other:?}"),
        }
    }

    #[test]
    fn test_http_auth_header_strips_bearer_case_insensitively() {
        let cases = [
            ("Bearer token123", "token123"),
            ("bearer token123", "token123"),
            ("BEARER token123", "token123"),
            ("bEaReR token123", "token123"),
            // Non-Bearer scheme: pass through verbatim. rmcp will then prefix
            // with "Bearer ", which is the contract for non-bearer auth too.
            ("Token foo", "Token foo"),
            ("token123", "token123"),
        ];

        for (input, expected) in cases {
            let server = acp::McpServer::Http(
                acp::McpServerHttp::new("http-server", "https://example.com/mcp")
                    .headers(vec![acp::HttpHeader::new("Authorization", input)]),
            );
            let configs = map_acp_mcp_servers(vec![server]);
            match &configs[0].transport {
                McpTransport::Http { config } => {
                    assert_eq!(config.auth_header.as_deref(), Some(expected), "input was {input:?}");
                }
                other => panic!("Expected Http, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_map_acp_sse_server() {
        let server = acp::McpServer::Sse(acp::McpServerSse::new("sse-server", "https://example.com/sse"));

        let configs = map_acp_mcp_servers(vec![server]);
        assert_eq!(configs.len(), 1);

        match &configs[0].transport {
            McpTransport::Http { config } => {
                assert_eq!(configs[0].name, "sse-server");
                assert_eq!(config.uri.as_ref(), "https://example.com/sse");
                assert_eq!(config.auth_header, None);
            }
            other => panic!("Expected Http, got {other:?}"),
        }
    }

    #[test]
    fn test_humanize_tool_name() {
        assert_eq!(humanize_tool_name("coding__read_file"), "Read file");
        assert_eq!(humanize_tool_name("read_file"), "Read file");
        assert_eq!(humanize_tool_name("bash"), "Bash");
        assert_eq!(humanize_tool_name("plugins__coding__read_file"), "Read file");
    }

    #[test]
    fn test_result_with_result_meta_sets_meta() {
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
        match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => {
                assert_eq!(update.fields.title.as_deref(), Some("Read file"), "native title should be set");
                let meta = update.meta.expect("meta should be present");
                assert_eq!(
                    meta.get("display_value").and_then(|v| v.as_str()),
                    Some("Cargo.toml, 156 lines"),
                    "display_value should be a flat key in _meta"
                );
                assert!(meta.get("display").is_none(), "old nested display object should not be in _meta");
            }
            other => panic!("Expected ToolCallUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_result_without_result_meta() {
        let session_id = acp::SessionId::new("test-session");
        let result = ToolCallResult {
            id: "call_1".to_string(),
            name: "external__some_tool".to_string(),
            arguments: "{}".to_string(),
            result: "ok".to_string(),
        };

        let notification = map_tool_result_to_notification(session_id, &result, None);
        match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => {
                assert!(update.fields.title.is_none());
                assert!(update.meta.is_none());
            }
            other => panic!("Expected ToolCallUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_plan_notification_extracted_from_result_meta() {
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

        let notification = try_extract_plan_notification(session_id, Some(&meta)).expect("should produce plan");
        match notification.update {
            acp::SessionUpdate::Plan(plan) => {
                assert_eq!(plan.entries.len(), 2);
                assert_eq!(plan.entries[0].content, "Research AI agents");
                assert_eq!(plan.entries[0].status, acp::PlanEntryStatus::InProgress);
                assert_eq!(plan.entries[1].content, "Write tests");
                assert_eq!(plan.entries[1].status, acp::PlanEntryStatus::Pending);
            }
            other => panic!("Expected Plan, got {other:?}"),
        }
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
    fn test_tool_progress_with_display_meta_emits_meta_update() {
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
            .expect("should produce notification");

        match notification.update {
            acp::SessionUpdate::ToolCallUpdate(update) => {
                assert_eq!(&*update.tool_call_id.0, "call_789");
                assert_eq!(update.fields.title.as_deref(), Some("Read file"), "native title should be set");
                let meta_map = update.meta.expect("meta should be present");
                assert_eq!(
                    meta_map.get("display_value").and_then(|v| v.as_str()),
                    Some("main.rs"),
                    "display_value should be a flat key in _meta"
                );
                assert!(meta_map.get("display").is_none(), "old nested display object should not be in _meta");
                assert_eq!(update.fields.status, Some(acp::ToolCallStatus::InProgress));
                // Should NOT have content (no text progress fallback)
                assert!(update.fields.content.is_none());
            }
            other => panic!("Expected ToolCallUpdate, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replay_to_client_forwards_each_event_as_session_notification() {
        use acp_utils::testing::test_connection;
        use llm::ContentBlock as LlmContentBlock;
        use tokio::task::LocalSet;

        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let session_id = acp::SessionId::new("test-session");
                let events = vec![SessionEvent::User(UserEvent::Message {
                    content: vec![LlmContentBlock::text("hello"), LlmContentBlock::text("world")],
                })];

                replay_to_client(&events, &cx, &session_id).await;

                for _ in 0..2 {
                    let notif = peer.next_session_notification().await;
                    assert_eq!(notif.session_id, session_id);
                    assert!(matches!(notif.update, SessionUpdate::UserMessageChunk(_)));
                }
            })
            .await;
    }
}
