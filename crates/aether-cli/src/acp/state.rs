use acp_utils::notifications::{
    AetherCapabilities, AuthMethodsUpdatedParams, McpRequest, PromptSearchParams, PromptSearchResponse,
    SessionDisplayMeta, SessionPreviewParams, SessionPreviewResponse, WorkspaceListParams, WorkspaceListResponse,
    WorkspaceMoveParams, WorkspaceMoveResponse,
};
use acp_utils::server::AcpServerError;
use aether_auth::OAuthCredentialStorage;
use aether_telemetry::TelemetryRuntime;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    self as acp, AgentCapabilities, AuthMethod, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    ConfigOptionUpdate, Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, McpCapabilities, NewSessionRequest,
    NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse, SessionId, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};
use agent_client_protocol::{Client, ConnectionTo, Responder};
use llm::catalog::{LlmModel, ModelSpec, get_local_models};
use llm::{ContentBlock, ProviderConnectionOverrides};
use mcp_utils::client::{client_capabilities, client_capabilities_for};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::task::spawn_blocking;
use tracing::{error, info};

use super::config_setting::ConfigSetting;
use super::model_config::supports_prompt_audio;
use super::protocol::content::map_acp_to_content_blocks;
use super::protocol::replay::replay_to_client;
use super::session_actor::{ConfigSnapshot, SessionCommand, SessionHandle};
use super::session_factory::SessionFactory;
use crate::resolve::InitialSessionSelection;
use crate::settings_args::SettingsSourceArgs;
use crate::workspace::{WorkspaceError, WorkspaceManager};
use aether_sessions::{SessionStore, SessionStoreError};

/// Global, connection-scoped ACP server state: the live session map plus the
/// dependencies needed to create sessions and answer global requests. There is
/// exactly one owner of mutable per-session state — the session actor behind
/// each [`SessionHandle`].
pub(crate) struct AcpState {
    sessions: Mutex<HashMap<String, SessionHandle>>,
    session_store: Arc<SessionStore>,
    workspace_manager: Arc<WorkspaceManager>,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    factory: SessionFactory,
    telemetry: Option<Arc<TelemetryRuntime>>,
    mcp_capabilities: Mutex<rmcp::model::ClientCapabilities>,
}

pub(crate) struct AcpStateConfig {
    pub(crate) session_store: Arc<SessionStore>,
    pub(crate) workspace_manager: Arc<WorkspaceManager>,
    pub(crate) oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    pub(crate) initial_selection: InitialSessionSelection,
    pub(crate) settings_source: SettingsSourceArgs,
    pub(crate) provider_connections: ProviderConnectionOverrides,
    pub(crate) telemetry: Option<Arc<TelemetryRuntime>>,
}

impl AcpState {
    pub(crate) fn new(config: AcpStateConfig) -> Self {
        let factory = SessionFactory::new(
            config.settings_source,
            config.provider_connections,
            Arc::clone(&config.oauth_credential_store),
            Arc::clone(&config.session_store),
            config.initial_selection,
            config.telemetry.as_ref().map(|runtime| runtime.observer_factory()),
        );
        Self {
            sessions: Mutex::new(HashMap::new()),
            session_store: config.session_store,
            workspace_manager: config.workspace_manager,
            oauth_credential_store: config.oauth_credential_store,
            factory,
            telemetry: config.telemetry,
            mcp_capabilities: Mutex::new(client_capabilities()),
        }
    }

    pub(crate) async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse, acp::Error> {
        info!("Received initialize request: {:?}", args);
        *self.mcp_capabilities.lock().await = mcp_client_capabilities(&args.client_capabilities);
        let auth_methods = build_auth_methods(self.oauth_credential_store.as_ref());
        let available = get_local_models().await;
        let prompt_capabilities = prompt_capabilities_for_models(&available);
        let aether_capabilities =
            AetherCapabilities { prompt_search: true, session_preview: true, workspace_move: true };
        let session_capabilities = acp::SessionCapabilities::new()
            .list(acp::SessionListCapabilities::new())
            .meta(Some(aether_capabilities.to_meta()));

        Ok(InitializeResponse::new(ProtocolVersion::V1)
            .agent_info(Implementation::new("Aether", "0.1.0"))
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .mcp_capabilities(McpCapabilities::new().http(true).sse(true))
                    .session_capabilities(session_capabilities)
                    .prompt_capabilities(prompt_capabilities),
            )
            .auth_methods(auth_methods))
    }

    pub(crate) async fn authenticate(
        &self,
        args: AuthenticateRequest,
        cx: &ConnectionTo<Client>,
    ) -> Result<AuthenticateResponse, acp::Error> {
        info!("Received authenticate request: {:?}", args);
        let method_id = args.method_id.0.as_ref();
        match method_id {
            "codex" => {
                llm::perform_codex_oauth_flow(self.oauth_credential_store.as_ref()).await.map_err(|e| {
                    error!("OAuth flow failed for {method_id}: {e}");
                    acp::Error::internal_error()
                })?;
            }
            _ => return Err(acp::Error::invalid_params()),
        }
        let auth_methods = build_auth_methods(self.oauth_credential_store.as_ref());
        if let Err(e) = cx
            .send_notification(AuthMethodsUpdatedParams { auth_methods })
            .map_err(|e| AcpServerError::protocol("_aether/auth_methods_updated", e))
        {
            error!("Failed to send auth methods updated notification: {:?}", e);
        }

        self.broadcast_config_options(cx).await;
        Ok(AuthenticateResponse::default())
    }

    pub(crate) async fn new_session(
        &self,
        req: NewSessionRequest,
        cx: &ConnectionTo<Client>,
    ) -> Result<NewSessionResponse, acp::Error> {
        let mcp_capabilities = self.mcp_capabilities.lock().await.clone();
        let created = self.factory.create(req, cx, mcp_capabilities).await?;
        let response = NewSessionResponse::new(created.session_id.clone()).config_options(created.config_options);
        self.register_session(&created.session_id, created.handle).await;
        Ok(response)
    }

    pub(crate) async fn load_session(
        &self,
        req: LoadSessionRequest,
        cx: &ConnectionTo<Client>,
    ) -> Result<LoadSessionResponse, acp::Error> {
        let mcp_capabilities = self.mcp_capabilities.lock().await.clone();
        let created = self.factory.load(req, cx, mcp_capabilities).await?;
        let response = LoadSessionResponse::new().config_options(created.config_options);
        self.register_session(&created.session_id, created.handle).await;
        replay_to_client(&created.replay_events, cx, &created.session_id).await;
        Ok(response)
    }

    pub(crate) fn list_sessions(&self, args: &ListSessionsRequest) -> ListSessionsResponse {
        info!("Listing sessions, cwd filter: {:?}", args.cwd);
        let mut summaries = self.session_store.list();

        if let Some(cwd) = args.cwd.as_ref() {
            summaries.retain(|s| s.meta.cwd == *cwd);
        }

        let sessions: Vec<acp::SessionInfo> = summaries
            .into_iter()
            .map(|s| {
                acp::SessionInfo::new(s.meta.session_id, s.meta.cwd)
                    .updated_at(s.meta.created_at)
                    .title(s.title)
                    .meta(SessionDisplayMeta::new(s.meta.model, s.meta.selected_mode).to_meta())
            })
            .collect();

        info!("Found {} sessions", sessions.len());
        ListSessionsResponse::new(sessions)
    }

    pub(crate) fn search_prompts(&self, params: &PromptSearchParams) -> Result<PromptSearchResponse, acp::Error> {
        self.session_store.search_prompts(params).map_err(|e| {
            error!("Prompt search failed: {e}");
            acp::Error::internal_error()
        })
    }

    pub(crate) fn session_preview(&self, params: &SessionPreviewParams) -> Result<SessionPreviewResponse, acp::Error> {
        self.session_store.preview(params).map_err(|e| {
            error!("Session preview failed: {e}");
            match e {
                SessionStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    acp::Error::invalid_params()
                }
                _ => acp::Error::internal_error(),
            }
        })
    }

    /// Lists managed workspaces sharing the session's repository.
    pub(crate) async fn workspace_list(
        &self,
        params: &WorkspaceListParams,
    ) -> Result<WorkspaceListResponse, acp::Error> {
        self.run_workspace_request(params.session_id.clone(), |_session_store, manager, src_cwd| {
            manager
                .list(&src_cwd)
                .map(|workspaces| WorkspaceListResponse { workspaces })
                .map_err(WorkspaceRequestError::Workspace)
        })
        .await
    }

    /// Moves the session's uncommitted changes to the target workspace, then
    /// relocates the stored session to the target directory.
    pub(crate) async fn workspace_move(
        &self,
        params: &WorkspaceMoveParams,
    ) -> Result<WorkspaceMoveResponse, acp::Error> {
        let target = params.target.clone();
        let session_id = params.session_id.clone();
        let response = self
            .run_workspace_request(session_id.clone(), move |session_store, manager, src_cwd| {
                let new_cwd = manager.move_to(&src_cwd, &target).map_err(WorkspaceRequestError::Workspace)?;
                session_store.relocate(&session_id, &new_cwd).map_err(WorkspaceRequestError::Relocate)?;
                Ok(WorkspaceMoveResponse { new_cwd })
            })
            .await?;

        info!("Moved session {} to workspace {}", params.session_id, response.new_cwd.display());
        Ok(response)
    }

    async fn run_workspace_request<T, F>(&self, session_id: String, op: F) -> Result<T, acp::Error>
    where
        T: Send + 'static,
        F: FnOnce(&SessionStore, &WorkspaceManager, PathBuf) -> Result<T, WorkspaceRequestError> + Send + 'static,
    {
        let session_store = Arc::clone(&self.session_store);
        let manager = Arc::clone(&self.workspace_manager);
        spawn_blocking(move || {
            let src_cwd = session_store
                .session_cwd(&session_id)
                .ok_or_else(|| WorkspaceRequestError::UnknownSession(session_id.clone()))?;
            op(&session_store, &manager, src_cwd)
        })
        .await
        .map_err(|e| internal_error(format!("workspace task failed: {e}")))?
        .map_err(workspace_request_error)
    }

    /// Route a prompt to its session actor. Validates media support against the
    /// session's effective model before dispatch, then hands the responder to
    /// the actor which answers when the turn completes.
    pub(crate) async fn route_prompt(&self, args: PromptRequest, responder: Responder<PromptResponse>) {
        info!("Received prompt for session: {:?}", args.session_id);
        let session_id = args.session_id.0.to_string();
        let content = map_acp_to_content_blocks(args.prompt);

        let Some((sender, snapshot)) = self.lookup(&session_id).await else {
            error!("Session not found: {session_id}");
            respond_err(responder, acp::Error::invalid_params());
            return;
        };

        if let Err(e) = validate_prompt_support(&snapshot.effective_model, &content) {
            respond_err(responder, e);
            return;
        }

        if let Err(SessionCommand::Prompt { responder, .. }) =
            sender.send(SessionCommand::Prompt { content, responder }).await.map_err(|e| e.0)
        {
            error!("Session actor channel closed for prompt: {session_id}");
            respond_err(responder, acp::Error::internal_error());
        }
    }

    pub(crate) async fn cancel(&self, args: CancelNotification) -> Result<(), acp::Error> {
        info!("Received cancel for session: {:?}", args.session_id);
        let session_id = args.session_id.0.to_string();
        let Some((sender, _)) = self.lookup(&session_id).await else {
            error!("Session not found for cancel: {session_id}");
            return Err(acp::Error::invalid_params());
        };
        sender.send(SessionCommand::Cancel).await.map_err(|_| {
            error!("Session actor channel closed for cancel: {session_id}");
            acp::Error::internal_error()
        })
    }

    /// Route a config change to its session actor. Discovers available models
    /// once, then the actor applies the change and answers with rebuilt options.
    pub(crate) async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
        responder: Responder<SetSessionConfigOptionResponse>,
    ) {
        let session_id = args.session_id.0.to_string();
        let config_id = args.config_id.0.to_string();
        let value = match args.value {
            acp::SessionConfigOptionValue::ValueId { value } => value.0.to_string(),
            acp::SessionConfigOptionValue::Boolean { value } => value.to_string(),
            _ => {
                respond_err(responder, acp::Error::invalid_params());
                return;
            }
        };
        info!("set_session_config_option: session={session_id}, config={config_id}, value={value}");

        let setting = match ConfigSetting::parse(&config_id, &value) {
            Ok(setting) => setting,
            Err(e) => {
                error!("{e}");
                respond_err(responder, acp::Error::invalid_params());
                return;
            }
        };

        let Some((sender, _)) = self.lookup(&session_id).await else {
            error!("Session not found: {session_id}");
            respond_err(responder, acp::Error::invalid_params());
            return;
        };

        let available = get_local_models().await;
        if let Err(SessionCommand::SetConfig { responder, .. }) =
            sender.send(SessionCommand::SetConfig { setting, available, responder }).await.map_err(|e| e.0)
        {
            error!("Session actor channel closed for set_config: {session_id}");
            respond_err(responder, acp::Error::internal_error());
        }
    }

    pub(crate) async fn on_mcp_request(&self, request: McpRequest) -> Result<(), acp::Error> {
        info!("Received MCP ext request: {:?}", request);
        match request {
            McpRequest::Authenticate { session_id, server_name } => {
                let Some((sender, _)) = self.lookup(&session_id).await else {
                    error!("Session not found for authenticate_mcp_server: {session_id}");
                    return Err(acp::Error::invalid_params());
                };
                sender.send(SessionCommand::AuthenticateMcp { server_name }).await.map_err(|_| {
                    error!("Session actor channel closed for MCP auth: {session_id}");
                    acp::Error::internal_error()
                })?;
            }
        }
        Ok(())
    }

    /// Drain every session and stop its actor task. Fans out cancellation before
    /// awaiting any join so shutdowns run concurrently.
    pub(crate) async fn shutdown_all(&self) {
        let handles: Vec<SessionHandle> = self.sessions.lock().await.drain().map(|(_, handle)| handle).collect();
        for handle in &handles {
            handle.cancel();
        }
        futures::future::join_all(handles.into_iter().map(SessionHandle::join)).await;
        if let Some(telemetry) = &self.telemetry {
            telemetry.shutdown_or_log();
        }
    }

    pub(crate) async fn register_session(&self, session_id: &SessionId, handle: SessionHandle) {
        if let Some(old) = self.sessions.lock().await.insert(session_id.0.to_string(), handle) {
            old.cancel();
        }
    }

    async fn lookup(&self, session_id: &str) -> Option<(mpsc::Sender<SessionCommand>, ConfigSnapshot)> {
        let sessions = self.sessions.lock().await;
        let handle = sessions.get(session_id)?;
        Some((handle.command_sender(), handle.config_snapshot()))
    }

    async fn broadcast_config_options(&self, cx: &ConnectionTo<Client>) {
        let available = get_local_models().await;
        let snapshots: Vec<(String, ConfigSnapshot)> = {
            let sessions = self.sessions.lock().await;
            sessions.iter().map(|(id, handle)| (id.clone(), handle.config_snapshot())).collect()
        };

        for (id, snapshot) in snapshots {
            let options = snapshot.config_options(&available, self.oauth_credential_store.as_ref());
            let notification = SessionNotification::new(
                SessionId::new(id),
                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
            );
            let _ = cx.send_notification(notification);
        }
    }
}

fn respond_err<T: agent_client_protocol::JsonRpcResponse>(responder: Responder<T>, error: acp::Error) {
    if let Err(e) = responder.respond_with_error(error) {
        error!("failed to send error response: {e:?}");
    }
}

enum WorkspaceRequestError {
    UnknownSession(String),
    Workspace(WorkspaceError),
    Relocate(SessionStoreError),
}

fn workspace_request_error(e: WorkspaceRequestError) -> acp::Error {
    match e {
        WorkspaceRequestError::UnknownSession(session_id) => {
            invalid_params_error(format!("unknown session: {session_id}"))
        }
        WorkspaceRequestError::Workspace(e) => workspace_error(&e),
        WorkspaceRequestError::Relocate(e) => internal_error(format!("failed to relocate session: {e}")),
    }
}

fn workspace_error(e: &WorkspaceError) -> acp::Error {
    if e.is_invalid_input() { invalid_params_error(e.to_string()) } else { internal_error(e.to_string()) }
}

fn invalid_params_error(message: impl Into<String>) -> acp::Error {
    acp::Error::new(i32::from(acp::ErrorCode::InvalidParams), message)
}

fn internal_error(message: impl Into<String>) -> acp::Error {
    acp::Error::new(i32::from(acp::ErrorCode::InternalError), message)
}

fn mcp_client_capabilities(client: &acp::ClientCapabilities) -> rmcp::model::ClientCapabilities {
    let elicitation = client.elicitation.as_ref();
    client_capabilities_for(
        elicitation.is_some_and(|capabilities| capabilities.form.is_some()),
        elicitation.is_some_and(|capabilities| capabilities.url.is_some()),
    )
}

fn build_auth_methods(store: &dyn OAuthCredentialStorage) -> Vec<AuthMethod> {
    let mut seen = HashSet::new();
    LlmModel::all()
        .iter()
        .filter_map(LlmModel::oauth_provider_id)
        .filter(|id| seen.insert(*id))
        .map(|id| {
            let display = LlmModel::all()
                .iter()
                .find(|m| m.oauth_provider_id() == Some(id))
                .map_or(id, |m| m.provider_display_name());
            let mut method = acp::AuthMethodAgent::new(id, display);
            if store.contains(id) {
                method = method.description("authenticated");
            }
            AuthMethod::Agent(method)
        })
        .collect()
}

fn prompt_capabilities_for_models(models: &[LlmModel]) -> PromptCapabilities {
    PromptCapabilities::new()
        .embedded_context(true)
        .image(models.iter().any(LlmModel::supports_image))
        .audio(models.iter().any(supports_prompt_audio))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PromptModalities {
    image: bool,
    audio: bool,
}

impl PromptModalities {
    fn from_content(content: &[ContentBlock]) -> Self {
        Self {
            image: content.iter().any(ContentBlock::is_image),
            audio: content.iter().any(|block| matches!(block, ContentBlock::Audio { .. })),
        }
    }

    fn is_empty(self) -> bool {
        !self.image && !self.audio
    }
}

fn selected_models(model_value: &str) -> Result<Vec<LlmModel>, acp::Error> {
    model_value.parse::<ModelSpec>().map(|spec| spec.models().to_vec()).map_err(|_| acp::Error::invalid_params())
}

fn validate_prompt_support(model_value: &str, content: &[ContentBlock]) -> Result<(), acp::Error> {
    let modalities = PromptModalities::from_content(content);
    if modalities.is_empty() {
        return Ok(());
    }

    let selected = selected_models(model_value)?;
    if modalities.image && selected.iter().any(|model| !model.supports_image()) {
        return Err(acp::Error::invalid_params());
    }
    if modalities.audio && selected.iter().any(|model| !supports_prompt_audio(model)) {
        return Err(acp::Error::invalid_params());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_sessions::SessionStore;

    const SONNET: &str = "anthropic:claude-sonnet-4-5";
    const DEEPSEEK: &str = "deepseek:deepseek-chat";
    const AUDIO_ONLY: &str = "bedrock:mistral.voxtral-small-24b-2507";

    fn fake_oauth_store() -> Arc<dyn OAuthCredentialStorage> {
        Arc::new(aether_auth::FakeOAuthCredentialStore::new())
    }

    fn test_state() -> AcpState {
        let session_store = Arc::new(SessionStore::from_path(PathBuf::from("/tmp/aether-test-sessions")));
        let workspace_manager =
            Arc::new(WorkspaceManager::from_registry_path(PathBuf::from("/tmp/aether-test-workspaces.json")));
        AcpState::new(AcpStateConfig {
            session_store,
            workspace_manager,
            oauth_credential_store: fake_oauth_store(),
            initial_selection: InitialSessionSelection::default(),
            settings_source: SettingsSourceArgs::default(),
            provider_connections: ProviderConnectionOverrides::default(),
            telemetry: None,
        })
    }

    #[tokio::test]
    async fn initialize_always_advertises_load_session_support() {
        let state = test_state();
        let response =
            state.initialize(InitializeRequest::new(ProtocolVersion::V1)).await.expect("initialize succeeds");
        let json = serde_json::to_string(&response).expect("response serializes");
        assert!(json.contains("\"loadSession\":true"));
    }

    #[tokio::test]
    async fn initialize_advertises_aether_capabilities_once() {
        let state = test_state();
        let response =
            state.initialize(InitializeRequest::new(ProtocolVersion::V1)).await.expect("initialize succeeds");
        assert_eq!(
            AetherCapabilities::from_meta(response.agent_capabilities.prompt_capabilities.meta.as_ref()),
            AetherCapabilities::default()
        );
        let capabilities =
            AetherCapabilities::from_meta(response.agent_capabilities.session_capabilities.meta.as_ref());
        assert!(capabilities.prompt_search);
        assert!(capabilities.session_preview);
        assert!(capabilities.workspace_move);
    }

    #[test]
    fn mcp_elicitation_capabilities_mirror_the_acp_client() {
        let none = mcp_client_capabilities(&acp::ClientCapabilities::new());
        assert!(none.elicitation.is_none());

        let form_only = acp::ClientCapabilities::new()
            .elicitation(acp::ElicitationCapabilities::new().form(acp::ElicitationFormCapabilities::new()));
        let form = mcp_client_capabilities(&form_only).elicitation.unwrap();
        assert!(form.form.is_some());
        assert!(form.url.is_none());

        let url_only = acp::ClientCapabilities::new()
            .elicitation(acp::ElicitationCapabilities::new().url(acp::ElicitationUrlCapabilities::new()));
        let url = mcp_client_capabilities(&url_only).elicitation.unwrap();
        assert!(url.form.is_none());
        assert!(url.url.is_some());
    }

    #[test]
    fn prompt_capabilities_reflect_available_modalities() {
        let image_only = prompt_capabilities_for_models(&[SONNET.parse().unwrap()]);
        assert!(image_only.image);
        assert!(!image_only.audio);

        let audio_capable = prompt_capabilities_for_models(&[AUDIO_ONLY.parse().unwrap()]);
        assert!(!audio_capable.image);
        assert!(audio_capable.audio);

        let text_only = prompt_capabilities_for_models(&[DEEPSEEK.parse().unwrap()]);
        assert!(!text_only.image);
        assert!(!text_only.audio);
    }

    #[test]
    fn validate_prompt_support_requires_all_selected_models_to_support_media() {
        let image_content = vec![ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() }];
        let audio_content =
            vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }];

        assert!(validate_prompt_support(SONNET, &image_content).is_ok());
        assert!(validate_prompt_support(DEEPSEEK, &image_content).is_err());
        assert!(validate_prompt_support(AUDIO_ONLY, &audio_content).is_ok());
        assert!(validate_prompt_support(SONNET, &audio_content).is_err());
        assert!(validate_prompt_support(format!("{SONNET},{DEEPSEEK}").as_str(), &image_content).is_err());
        assert!(validate_prompt_support(format!("{AUDIO_ONLY},{DEEPSEEK}").as_str(), &audio_content).is_err());
    }
}
