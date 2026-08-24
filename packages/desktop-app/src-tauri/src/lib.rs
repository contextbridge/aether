use acp_utils::client::AcpEvent;
#[cfg(debug_assertions)]
use specta_typescript::Typescript;
use std::sync::Arc;
use tauri_specta::{Builder, ErrorHandlingMode, collect_commands};
mod agent_session;
mod app_event;
mod app_state;
mod commands;
mod files;
use app_event::AppEvent;
use app_state::AppState;
#[cfg(test)]
use files::build_file_content_blocks;
#[cfg(test)]
use files::{FileEntry, collect_workspace_files};

fn bridge_event(session_id: String, connection_id: String, event: AcpEvent) -> Option<AppEvent> {
    match event {
        AcpEvent::SessionUpdate { session_id, update } => {
            Some(AppEvent::SessionUpdate { session_id: session_id.0.to_string(), connection_id, update: *update })
        }
        AcpEvent::PromptDone(stop_reason) => {
            Some(AppEvent::PromptDone { session_id, connection_id, stop_reason: format!("{stop_reason:?}") })
        }
        AcpEvent::PromptError(error) => {
            Some(AppEvent::PromptError { session_id, connection_id, error: error.to_string() })
        }
        _ => None,
    }
}

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::new()
        .error_handling(ErrorHandlingMode::Throw)
        .commands(collect_commands![
            commands::start_session,
            commands::set_session_config_option,
            commands::send_prompt,
            commands::index_workspace_files,
            commands::cancel_prompt,
            commands::close_session
        ])
        .typ::<AppEvent>()
}

#[cfg(debug_assertions)]
fn export_bindings() {
    specta_builder()
        .export(
            Typescript::default()
                .header("import type { SessionConfigOption, SessionUpdate } from \"@agentclientprotocol/sdk\";"),
            "../src/bindings.ts",
        )
        .expect("failed to export TypeScript bindings");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(debug_assertions)]
    export_bindings();

    let bindings = specta_builder();
    let state = Arc::new(AppState::default());
    let page_state = state.clone();
    tauri::Builder::default()
        .manage(state)
        .on_page_load(move |_webview, _payload| {
            let state = page_state.clone();
            tauri::async_runtime::spawn(async move {
                state.close_all_sessions().await;
            });
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(bindings.invoke_handler())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, Error as ProtocolError, MessageId, SessionId, SessionUpdate, StopReason,
        TextContent, ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    };

    fn connection_id() -> String {
        "conn-1".to_string()
    }

    fn text_chunk(text: &str, message_id: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::Text(TextContent::new(text))).message_id(MessageId::new(message_id)),
        )
    }

    #[cfg(debug_assertions)]
    #[test]
    fn exports_typescript_bindings() {
        export_bindings();
    }

    #[test]
    fn serializes_bridge_fields_for_the_renderer() {
        let value = serde_json::to_value(AppEvent::PromptDone {
            session_id: "s1".to_string(),
            connection_id: "conn-1".to_string(),
            stop_reason: "EndTurn".to_string(),
        })
        .unwrap();

        assert_eq!(value["connectionId"], "conn-1");
        assert_eq!(value["stopReason"], "EndTurn");
        assert!(value.get("connection_id").is_none());
        assert!(value.get("stop_reason").is_none());
    }

    #[test]
    fn forwards_session_update_without_loss() {
        let update = text_chunk("hello", "msg-1");
        let event = AcpEvent::SessionUpdate { session_id: SessionId::new("s1"), update: Box::new(update) };

        let bridge = bridge_event("s1".to_string(), connection_id(), event).expect("session update should forward");

        let AppEvent::SessionUpdate { connection_id, session_id, update } = bridge else {
            panic!("expected SessionUpdate, got {bridge:?}");
        };
        assert_eq!(connection_id, "conn-1");
        assert_eq!(session_id, "s1");
        let update = serde_json::to_value(update).unwrap();
        assert_eq!(update["sessionUpdate"], "agent_message_chunk");
        assert_eq!(update["messageId"], "msg-1");
        assert_eq!(update["content"]["type"], "text");
        assert_eq!(update["content"]["text"], "hello");
    }

    #[test]
    fn forwards_tool_call_with_raw_input_and_meta() {
        let mut meta = serde_json::Map::new();
        meta.insert("aetherToolName".into(), "coding__read_file".into());
        let call = ToolCall::new(ToolCallId::new("t1"), "Read file")
            .status(ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({ "path": "src/lib.rs" }))
            .meta(meta);
        let event = AcpEvent::SessionUpdate {
            session_id: SessionId::new("s1"),
            update: Box::new(SessionUpdate::ToolCall(call)),
        };

        let bridge = bridge_event("s1".to_string(), connection_id(), event).expect("tool call should forward");

        let AppEvent::SessionUpdate { update, .. } = bridge else { panic!("expected SessionUpdate") };
        let update = serde_json::to_value(update).unwrap();
        assert_eq!(update["sessionUpdate"], "tool_call");
        assert_eq!(update["toolCallId"], "t1");
        assert_eq!(update["title"], "Read file");
        assert_eq!(update["status"], "in_progress");
        assert_eq!(update["rawInput"]["path"], "src/lib.rs");
        assert_eq!(update["_meta"]["aetherToolName"], "coding__read_file");
    }

    #[test]
    fn forwards_tool_call_update_with_status_and_content() {
        let fields = ToolCallUpdateFields::new().status(ToolCallStatus::Completed).title("Read file");
        let update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(ToolCallId::new("t1"), fields));
        let event = AcpEvent::SessionUpdate { session_id: SessionId::new("s1"), update: Box::new(update) };

        let bridge = bridge_event("s1".to_string(), connection_id(), event).expect("tool call update should forward");

        let AppEvent::SessionUpdate { update, .. } = bridge else { panic!("expected SessionUpdate") };
        let update = serde_json::to_value(update).unwrap();
        assert_eq!(update["sessionUpdate"], "tool_call_update");
        assert_eq!(update["toolCallId"], "t1");
        assert_eq!(update["status"], "completed");
        assert_eq!(update["title"], "Read file");
    }

    #[test]
    fn maps_prompt_lifecycle_events() {
        let done = bridge_event("s1".to_string(), connection_id(), AcpEvent::PromptDone(StopReason::EndTurn));
        assert_eq!(
            done,
            Some(AppEvent::PromptDone {
                session_id: "s1".to_string(),
                connection_id: "conn-1".to_string(),
                stop_reason: "EndTurn".to_string(),
            })
        );

        let error =
            bridge_event("s1".to_string(), connection_id(), AcpEvent::PromptError(ProtocolError::new(0, "boom")));
        assert_eq!(
            error,
            Some(AppEvent::PromptError {
                session_id: "s1".to_string(),
                connection_id: "conn-1".to_string(),
                error: "boom".to_string(),
            })
        );
    }

    #[test]
    fn embeds_mentioned_text_files_as_acp_resources() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "fn main() {}\n").unwrap();

        let blocks = build_file_content_blocks(root.path(), &[path.to_string_lossy().into_owned()]).unwrap();

        assert_eq!(blocks.len(), 1);
        let value = serde_json::to_value(&blocks[0]).unwrap();
        assert_eq!(value["type"], "resource");
        assert_eq!(value["resource"]["text"], "fn main() {}\n");
        assert_eq!(value["resource"]["mimeType"], "text/x-rust");
    }

    #[test]
    fn indexes_workspace_files_for_mentions() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "hello").unwrap();
        std::fs::create_dir(root.path().join("node_modules")).unwrap();
        std::fs::write(root.path().join("node_modules/ignored.js"), "ignored").unwrap();

        let files = collect_workspace_files(root.path()).unwrap();

        assert_eq!(
            files,
            vec![FileEntry {
                path: root.path().join("README.md").to_string_lossy().into_owned(),
                display_name: "README.md".to_string(),
            }]
        );
    }

    #[test]
    fn ignores_unrelated_events() {
        let event = AcpEvent::SessionsListed { sessions: Vec::new() };
        assert_eq!(bridge_event("s1".to_string(), connection_id(), event), None);
    }
}
