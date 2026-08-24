use crate::AppEvent;
use crate::app_state::AppState;
use crate::files::{FileEntry, collect_workspace_files};
use agent_client_protocol::schema::v1::SessionConfigOption;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use std::sync::Arc;
use tauri::State;
use tauri::ipc::Channel;

#[derive(Debug, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartSessionRequest {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: String,
}

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionInfo {
    pub(crate) connection_id: String,
    pub(crate) session_id: String,
    pub(crate) agent_name: String,
    #[specta(type = Vec<AcpSessionConfigOptionType>)]
    pub(crate) config_options: Vec<SessionConfigOption>,
}

struct AcpSessionConfigOptionType;

impl Type for AcpSessionConfigOptionType {
    fn definition(_: &mut specta::Types) -> specta::datatype::DataType {
        specta::datatype::DataType::Reference(specta_typescript::define("SessionConfigOption"))
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn start_session(
    request: StartSessionRequest,
    events: Channel<AppEvent>,
    state: State<'_, Arc<AppState>>,
) -> Result<SessionInfo, String> {
    let session = state.inner().start_session(request.program, request.args, request.cwd, events).await?;

    Ok(SessionInfo {
        connection_id: session.connection_id,
        session_id: session.session_id.0.to_string(),
        agent_name: session.agent_name,
        config_options: session.config_options,
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_session_config_option(
    session_id: String,
    config_id: String,
    value: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    state.set_session_config_option(&session_id, &config_id, &value).await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn send_prompt(
    session_id: String,
    text: String,
    file_paths: Option<Vec<String>>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    state.send_prompt(&session_id, &text, file_paths.as_deref()).await
}

#[tauri::command]
#[specta::specta]
pub(crate) fn index_workspace_files(cwd: &str) -> Result<Vec<FileEntry>, String> {
    collect_workspace_files(Path::new(cwd))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cancel_prompt(session_id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.cancel_prompt(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn close_session(session_id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.close_session(&session_id).await
}
