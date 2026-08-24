use crate::AppEvent;
use crate::app_state::AppState;
use crate::files::{FileEntry, collect_workspace_files};
use crate::git::{DiffFileContents, DiffScope, FileStatus, GitRepository, GitSnapshot};
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
    state.set_session_config_option(&session_id, &config_id, &value)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn send_prompt(
    session_id: String,
    text: String,
    file_paths: Option<Vec<String>>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    state.send_prompt(&session_id, &text, file_paths.as_deref())
}

#[tauri::command]
#[specta::specta]
pub(crate) fn index_workspace_files(cwd: &str) -> Result<Vec<FileEntry>, String> {
    collect_workspace_files(Path::new(cwd))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cancel_prompt(session_id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.cancel_prompt(&session_id)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn close_session(session_id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.close_session(&session_id)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn load_git_snapshot(
    session_id: String,
    scope: DiffScope,
    state: State<'_, Arc<AppState>>,
) -> Result<GitSnapshot, String> {
    repository(&state, &session_id)?.snapshot(scope).await.map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn load_diff_files(
    session_id: String,
    path: String,
    old_path: Option<String>,
    scope: DiffScope,
    state: State<'_, Arc<AppState>>,
) -> Result<DiffFileContents, String> {
    repository(&state, &session_id)?
        .load_file_contents(&path, old_path.as_deref(), scope)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn stage_git_paths(
    session_id: String,
    paths: Vec<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let _guard = state.lock_git_mutations().await;
    repository(&state, &session_id)?.stage(&paths).await.map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn unstage_git_paths(
    session_id: String,
    paths: Vec<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let _guard = state.lock_git_mutations().await;
    repository(&state, &session_id)?.unstage(&paths).await.map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn stage_all_git_changes(session_id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let _guard = state.lock_git_mutations().await;
    repository(&state, &session_id)?.stage_all().await.map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn unstage_all_git_changes(session_id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let _guard = state.lock_git_mutations().await;
    repository(&state, &session_id)?.unstage_all().await.map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_git_changes(
    session_id: String,
    message: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let _guard = state.lock_git_mutations().await;
    repository(&state, &session_id)?.commit(&message).await.map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn discard_git_path(
    session_id: String,
    path: String,
    old_path: Option<String>,
    status: FileStatus,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let _guard = state.lock_git_mutations().await;
    repository(&state, &session_id)?
        .discard(&path, old_path.as_deref(), status)
        .await
        .map_err(|error| error.to_string())
}

fn repository(state: &State<'_, Arc<AppState>>, session_id: &str) -> Result<GitRepository, String> {
    state.working_directory(session_id).map(GitRepository::new)
}
