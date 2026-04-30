use acp_utils::notifications::{AuthMethodsUpdatedParams, McpRequest};
use acp_utils::server::AcpServerError;
use agent_client_protocol::schema::{
    self as acp, AgentCapabilities, AuthMethod, AuthenticateRequest, AuthenticateResponse, AvailableCommandsUpdate,
    ConfigOptionUpdate, Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, McpCapabilities, NewSessionRequest,
    NewSessionResponse, PromptCapabilities, PromptResponse, ProtocolVersion, SessionId, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};
use agent_client_protocol::{Client, ConnectionTo};
use llm::catalog::{LlmModel, get_local_models};
use llm::oauth::OAuthCredentialStore;
use llm::types::IsoString;
use llm::{ContentBlock, ReasoningEffort};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::oneshot;
use tracing::{error, info, warn};

use super::config_setting::ConfigSetting;
use super::mappers::{map_acp_mcp_servers, replay_to_client};
use super::model_config::{
    ValidatedMode, build_config_options_from_modes, pick_default_model, supports_prompt_audio,
    validated_modes_from_specs,
};
use super::relay::{SessionCommand, spawn_relay};
use super::session::Session;
use super::session_registry::{ConfigSnapshot, SessionRegistry};
use super::session_store::{SessionMeta, SessionStore};
use crate::settings_args::SettingsSourceArgs;
use acp_utils::content::format_embedded_resource;
use aether_core::agent_spec::AgentSpec;
use aether_core::context::ext::ContextExt;
use aether_project::{AetherSettings, AgentCatalog};
use llm::Context;

/// Initial session selection supplied when `aether acp` starts.
#[derive(Clone, Debug, Default)]
pub enum InitialSessionSelection {
    #[default]
    Default,
    Agent(String),
    Model {
        model: String,
        reasoning_effort: Option<ReasoningEffort>,
    },
}

impl InitialSessionSelection {
    pub fn agent(name: String) -> Self {
        Self::Agent(name)
    }

    pub fn model(model: String, reasoning_effort: Option<ReasoningEffort>) -> Self {
        Self::Model { model, reasoning_effort }
    }
}

/// Manages ACP sessions, each session has its own agent and state
pub struct SessionManager {
    registry: Arc<SessionRegistry>,
    session_store: Arc<SessionStore>,
    has_oauth_credential: fn(&str) -> bool,
    initial_selection: InitialSessionSelection,
    settings_source: SettingsSourceArgs,
}

pub(crate) struct SessionManagerConfig {
    pub(crate) registry: Arc<SessionRegistry>,
    pub(crate) session_store: Arc<SessionStore>,
    pub(crate) has_oauth_credential: fn(&str) -> bool,
    pub(crate) initial_selection: InitialSessionSelection,
    pub(crate) settings_source: SettingsSourceArgs,
}

struct SessionModeCatalog {
    catalog: AgentCatalog,
    modes: Vec<ValidatedMode>,
    available: Vec<LlmModel>,
}

struct ResolvedInitialSession {
    spec: AgentSpec,
    selected_mode: Option<String>,
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

impl SessionManager {
    pub(crate) fn new(deps: SessionManagerConfig) -> Self {
        Self {
            registry: deps.registry,
            session_store: deps.session_store,
            has_oauth_credential: deps.has_oauth_credential,
            initial_selection: deps.initial_selection,
            settings_source: deps.settings_source,
        }
    }

    fn resolve_initial_session(
        &self,
        mode_catalog: &SessionModeCatalog,
        default_model: &LlmModel,
    ) -> Result<ResolvedInitialSession, acp::Error> {
        match &self.initial_selection {
            InitialSessionSelection::Default => resolve_default_initial_session(mode_catalog, default_model),
            InitialSessionSelection::Agent(agent) => {
                if !mode_catalog.modes.iter().any(|mode| mode.name == *agent) {
                    warn!("Unknown agent `{agent}` requested via --agent");
                    return Err(acp::Error::invalid_params());
                }
                resolve_agent_spec(&mode_catalog.catalog, agent)
                    .map(|spec| ResolvedInitialSession { spec, selected_mode: Some(agent.clone()) })
            }
            InitialSessionSelection::Model { model, reasoning_effort } => {
                let model = parse_available_model(model, &mode_catalog.available)?;
                Ok(ResolvedInitialSession {
                    spec: AgentSpec::default_spec(&model, *reasoning_effort, Vec::new()),
                    selected_mode: None,
                })
            }
        }
    }

    async fn load_mode_catalog(&self, cwd: &Path) -> Result<SessionModeCatalog, acp::Error> {
        let config = if let Some(source) = self.settings_source.source(cwd) {
            AetherSettings::load(cwd, [source])
        } else {
            AetherSettings::load_default(cwd)
        }
        .map_err(|e| {
            error!("Failed to load agent catalog: {e}");
            acp::Error::invalid_params()
        })?;
        let catalog = if config.agents.is_empty() {
            AgentCatalog::empty(cwd.to_path_buf())
        } else {
            AgentCatalog::from_settings(cwd, config).map_err(|e| {
                error!("Failed to load agent catalog: {e}");
                acp::Error::invalid_params()
            })?
        };

        let available = get_local_models().await;
        let specs: Vec<_> = catalog.user_invocable().cloned().collect();
        let modes = validated_modes_from_specs(&specs, &available);

        Ok(SessionModeCatalog { catalog, modes, available })
    }

    #[allow(clippy::too_many_arguments, clippy::similar_names)]
    async fn register_session(
        &self,
        session: Session,
        session_id: &str,
        acp_session_id: &SessionId,
        model: &str,
        selected_mode: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
        modes: Vec<ValidatedMode>,
        cx: &ConnectionTo<Client>,
    ) -> Vec<acp::SessionConfigOption> {
        let relay = spawn_relay(session, cx.clone(), acp_session_id.clone(), self.session_store.clone());

        self.registry
            .insert(
                session_id.to_string(),
                relay,
                model.to_string(),
                selected_mode.clone(),
                reasoning_effort,
                modes.clone(),
            )
            .await;

        let available = get_local_models().await;
        let all_models = get_all_models(&available);
        build_config_options_from_modes(
            &modes,
            &available,
            selected_mode.as_deref(),
            model,
            reasoning_effort,
            &all_models,
            &OAuthCredentialStore::default(),
        )
    }

    fn send_available_commands_notification(
        available_commands: Vec<acp::AvailableCommand>,
        acp_session_id: SessionId,
        session_id: &str,
        cx: &ConnectionTo<Client>,
    ) {
        if available_commands.is_empty() {
            return;
        }
        let command_count = available_commands.len();
        let notification = SessionNotification::new(
            acp_session_id,
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(available_commands)),
        );
        if let Err(e) = cx.send_notification(notification).map_err(|e| AcpServerError::protocol("session/update", e)) {
            error!("Failed to send available commands notification: {:?}", e);
        } else {
            info!("Sent available commands update for session {} ({} commands)", session_id, command_count);
        }
    }

    /// Drain every session and stop its relay task. Blocks until every relay
    /// has exited.
    pub async fn shutdown_all_sessions(&self) {
        self.registry.shutdown_all().await;
    }
}

fn options_from_snapshot(
    snapshot: &ConfigSnapshot,
    available: &[LlmModel],
    all_models: &[LlmModel],
    credential_store: &OAuthCredentialStore,
) -> Vec<acp::SessionConfigOption> {
    build_config_options_from_modes(
        &snapshot.modes,
        available,
        snapshot.selected_mode.as_deref(),
        &snapshot.effective_model,
        snapshot.reasoning_effort,
        all_models,
        credential_store,
    )
}

/// Merge catalog `all()` with locally-discovered models for the `all_models`
/// parameter of `build_model_config_option`.
fn get_all_models(discovered: &[LlmModel]) -> Vec<LlmModel> {
    let mut all = LlmModel::all().to_vec();
    for m in discovered {
        if !all.contains(m) {
            all.push(m.clone());
        }
    }
    all
}

fn build_auth_methods(has_credential: impl Fn(&str) -> bool) -> Vec<AuthMethod> {
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
            if has_credential(id) {
                method = method.description("authenticated");
            }
            AuthMethod::Agent(method)
        })
        .collect()
}

fn map_acp_to_content_blocks(blocks: Vec<acp::ContentBlock>) -> Vec<ContentBlock> {
    blocks
        .into_iter()
        .map(|block| match block {
            acp::ContentBlock::Text(t) => ContentBlock::text(t.text),
            acp::ContentBlock::Image(img) => ContentBlock::Image { data: img.data, mime_type: img.mime_type },
            acp::ContentBlock::Audio(aud) => ContentBlock::Audio { data: aud.data, mime_type: aud.mime_type },
            acp::ContentBlock::Resource(r) => ContentBlock::text(format_embedded_resource(&r)),
            acp::ContentBlock::ResourceLink(l) => ContentBlock::text(format!("[Resource: {}]", l.uri)),
            _ => ContentBlock::text("[Unknown content]"),
        })
        .collect()
}

fn resolve_agent_spec(catalog: &AgentCatalog, mode_name: &str) -> Result<AgentSpec, acp::Error> {
    catalog.resolve(mode_name).map_err(|e| {
        error!("Failed to resolve runtime inputs for mode '{}': {e}", mode_name);
        acp::Error::invalid_params()
    })
}

fn resolve_default_initial_session(
    mode_catalog: &SessionModeCatalog,
    default_model: &LlmModel,
) -> Result<ResolvedInitialSession, acp::Error> {
    if let Some(mode) = mode_catalog.modes.first() {
        return resolve_agent_spec(&mode_catalog.catalog, &mode.name)
            .map(|spec| ResolvedInitialSession { spec, selected_mode: Some(mode.name.clone()) });
    }

    Ok(ResolvedInitialSession { spec: AgentSpec::default_spec(default_model, None, Vec::new()), selected_mode: None })
}

fn parse_available_model(model: &str, available: &[LlmModel]) -> Result<LlmModel, acp::Error> {
    let parsed = model.parse().map_err(|e: String| {
        warn!("Failed to parse --model `{model}`: {e}");
        acp::Error::invalid_params()
    })?;
    if available.iter().any(|available| available == &parsed) {
        Ok(parsed)
    } else {
        warn!("Requested model `{model}` is not available");
        Err(acp::Error::invalid_params())
    }
}

fn prompt_capabilities_for_models(models: &[LlmModel]) -> PromptCapabilities {
    PromptCapabilities::new()
        .embedded_context(true)
        .image(models.iter().any(LlmModel::supports_image))
        .audio(models.iter().any(supports_prompt_audio))
}

fn selected_models(model_value: &str) -> Result<Vec<LlmModel>, acp::Error> {
    model_value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<LlmModel>().map_err(|_| acp::Error::invalid_params()))
        .collect()
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
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{InitializeRequest, ProtocolVersion};

    const SONNET: &str = "anthropic:claude-sonnet-4-5";
    const DEEPSEEK: &str = "deepseek:deepseek-chat";

    #[tokio::test]
    async fn initialize_always_advertises_load_session_support() {
        let session_store =
            SessionStore::new().map_or_else(|e| panic!("Failed to initialize session store: {e}"), Arc::new);
        let manager = SessionManager::new(SessionManagerConfig {
            registry: Arc::new(SessionRegistry::new()),
            session_store,
            has_oauth_credential: |_| false,
            initial_selection: InitialSessionSelection::default(),
            settings_source: SettingsSourceArgs::default(),
        });
        let response =
            manager.initialize(InitializeRequest::new(ProtocolVersion::LATEST)).await.expect("initialize succeeds");
        let json = serde_json::to_string(&response).expect("response serializes");
        assert!(json.contains("\"loadSession\":true"));
    }

    #[test]
    fn prompt_capabilities_reflect_available_modalities() {
        let image_only = prompt_capabilities_for_models(&["anthropic:claude-sonnet-4-5".parse().unwrap()]);
        assert!(image_only.image);
        assert!(!image_only.audio);

        let audio_capable =
            prompt_capabilities_for_models(&["gemini:gemini-live-2.5-flash-preview-native-audio".parse().unwrap()]);
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
        assert!(validate_prompt_support("gemini:gemini-live-2.5-flash-preview-native-audio", &audio_content,).is_ok());
        assert!(validate_prompt_support(SONNET, &audio_content).is_err());
        assert!(
            validate_prompt_support("anthropic:claude-sonnet-4-5,deepseek:deepseek-chat", &image_content,).is_err()
        );
        assert!(
            validate_prompt_support(
                "gemini:gemini-live-2.5-flash-preview-native-audio,deepseek:deepseek-chat",
                &audio_content,
            )
            .is_err()
        );
    }
}

impl SessionManager {
    pub async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse, acp::Error> {
        info!("Received initialize request: {:?}", args);
        let auth_methods = build_auth_methods(self.has_oauth_credential);
        let available = get_local_models().await;
        Ok(InitializeResponse::new(ProtocolVersion::V1)
            .agent_info(Implementation::new("Aether", "0.1.0"))
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .mcp_capabilities(McpCapabilities::new().http(true).sse(true))
                    .session_capabilities(acp::SessionCapabilities::new().list(acp::SessionListCapabilities::new()))
                    .prompt_capabilities(prompt_capabilities_for_models(&available)),
            )
            .auth_methods(auth_methods))
    }

    pub async fn authenticate(
        &self,
        args: AuthenticateRequest,
        cx: &ConnectionTo<Client>,
    ) -> Result<AuthenticateResponse, acp::Error> {
        info!("Received authenticate request: {:?}", args);
        let method_id = args.method_id.0.as_ref();
        match method_id {
            "codex" => {
                llm::perform_codex_oauth_flow().await.map_err(|e| {
                    error!("OAuth flow failed for {method_id}: {e}");
                    acp::Error::internal_error()
                })?;
            }
            _ => return Err(acp::Error::invalid_params()),
        }
        let auth_methods = build_auth_methods(self.has_oauth_credential);
        if let Err(e) = cx
            .send_notification(AuthMethodsUpdatedParams { auth_methods })
            .map_err(|e| AcpServerError::protocol("_aether/auth_methods_updated", e))
        {
            error!("Failed to send auth methods updated notification: {:?}", e);
        }

        let credential_store = OAuthCredentialStore::default();
        let available = get_local_models().await;
        let all_models = get_all_models(&available);
        let snapshots = self.registry.snapshot_all_configs().await;

        for (id, snap) in snapshots {
            let options = options_from_snapshot(&snap, &available, &all_models, &credential_store);
            let notification = SessionNotification::new(
                SessionId::new(id),
                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
            );
            let _ = cx.send_notification(notification);
        }

        Ok(AuthenticateResponse::default())
    }

    pub async fn new_session(
        &self,
        mut args: NewSessionRequest,
        cx: &ConnectionTo<Client>,
    ) -> Result<NewSessionResponse, acp::Error> {
        // Inside a sandbox container the client sends the *host* cwd, but the
        // project is mounted at the container's working directory.
        if std::env::var("AETHER_INSIDE_SANDBOX").is_ok() {
            let container_cwd = std::env::current_dir().unwrap_or_else(|_| "/workspace".into());
            info!("Sandbox: remapping cwd {:?} -> {:?}", args.cwd, container_cwd);
            args.cwd = container_cwd;
        }

        info!("Creating new session with cwd: {:?}", args.cwd);
        let session_id = uuid::Uuid::new_v4().to_string();
        let acp_session_id = acp::SessionId::new(session_id.clone());

        let mode_catalog = self.load_mode_catalog(&args.cwd).await?;
        let default_model = pick_default_model(&mode_catalog.available).ok_or_else(|| {
            error!("No models available — set an API key env var (e.g. ANTHROPIC_API_KEY)");
            acp::Error::internal_error()
        })?;

        let ResolvedInitialSession { spec, selected_mode } =
            self.resolve_initial_session(&mode_catalog, default_model)?;
        let model_str = spec.model.clone();
        let reasoning_effort = spec.reasoning_effort;

        let session =
            Session::new(spec, args.cwd.clone(), map_acp_mcp_servers(args.mcp_servers), None, Some(session_id.clone()))
                .await
                .map_err(|e| {
                    error!("Failed to create session: {}", e);
                    acp::Error::internal_error()
                })?;

        let available_commands = session.list_available_commands().await.map_err(|e| {
            error!("Failed to list available commands: {}", e);
            acp::Error::internal_error()
        })?;

        let meta = SessionMeta {
            session_id: session_id.clone(),
            cwd: args.cwd.clone(),
            model: model_str.clone(),
            selected_mode: selected_mode.clone(),
            created_at: IsoString::now().0,
        };
        if let Err(e) = self.session_store.append_meta(&session_id, &meta) {
            error!("Failed to write session meta: {e}");
        }

        let config_options = self
            .register_session(
                session,
                &session_id,
                &acp_session_id,
                &model_str,
                selected_mode,
                reasoning_effort,
                mode_catalog.modes,
                cx,
            )
            .await;

        info!("Session {} created successfully", session_id);

        let response = NewSessionResponse::new(acp_session_id.clone()).config_options(config_options);

        Self::send_available_commands_notification(available_commands, acp_session_id, &session_id, cx);

        Ok(response)
    }

    pub fn list_sessions(&self, args: &ListSessionsRequest) -> Result<ListSessionsResponse, acp::Error> {
        info!("Listing sessions, cwd filter: {:?}", args.cwd);
        let mut summaries = self.session_store.list();

        if let Some(cwd) = args.cwd.as_ref() {
            summaries.retain(|s| s.meta.cwd == *cwd);
        }

        let sessions: Vec<acp::SessionInfo> = summaries
            .into_iter()
            .map(|s| acp::SessionInfo::new(s.meta.session_id, s.meta.cwd).updated_at(s.meta.created_at).title(s.title))
            .collect();

        info!("Found {} sessions", sessions.len());
        Ok(ListSessionsResponse::new(sessions))
    }

    pub async fn load_session(
        &self,
        args: LoadSessionRequest,
        cx: &ConnectionTo<Client>,
    ) -> Result<LoadSessionResponse, acp::Error> {
        let session_id = args.session_id.0.to_string();
        info!("Loading session: {session_id}");

        let (meta, events) = self.session_store.load(&session_id).ok_or_else(|| {
            error!("Session not found: {session_id}");
            acp::Error::invalid_params()
        })?;

        let context = Context::from_events(&events);
        let mode_catalog = self.load_mode_catalog(&args.cwd).await?;

        let spec = if let Some(mode_name) = meta.selected_mode.as_deref() {
            resolve_agent_spec(&mode_catalog.catalog, mode_name)?
        } else {
            let parsed_model: LlmModel = meta.model.parse().map_err(|e: String| {
                error!("Failed to parse restored model '{}': {e}", meta.model);
                acp::Error::invalid_params()
            })?;
            AgentSpec::default_spec(&parsed_model, None, Vec::new())
        };

        let model = spec.model.clone();

        let restored_messages: Vec<_> = context.messages().iter().filter(|m| !m.is_system()).cloned().collect();

        let session = Session::new(
            spec,
            args.cwd.clone(),
            map_acp_mcp_servers(args.mcp_servers),
            Some(restored_messages),
            Some(session_id.clone()),
        )
        .await
        .map_err(|e| {
            error!("Failed to create session for load: {e}");
            acp::Error::internal_error()
        })?;

        let available_commands = session.list_available_commands().await.map_err(|e| {
            error!("Failed to list available commands: {e}");
            acp::Error::internal_error()
        })?;

        let acp_session_id = acp::SessionId::new(session_id.clone());

        let config_options = self
            .register_session(
                session,
                &session_id,
                &acp_session_id,
                &model,
                meta.selected_mode,
                None,
                mode_catalog.modes,
                cx,
            )
            .await;

        info!("Session {session_id} loaded successfully");

        let response = LoadSessionResponse::new().config_options(config_options);

        let cx_clone = cx.clone();
        let replay_session_id = acp_session_id.clone();
        spawn(async move {
            replay_to_client(&events, &cx_clone, &replay_session_id).await;
        });

        Self::send_available_commands_notification(available_commands, acp_session_id, &session_id, cx);

        Ok(response)
    }

    pub async fn prompt(&self, args: acp::PromptRequest) -> Result<acp::PromptResponse, acp::Error> {
        info!("Received prompt for session: {:?}", args.session_id);
        let session_id_str = args.session_id.0.to_string();
        let content = map_acp_to_content_blocks(args.prompt);

        let model = self.registry.effective_model(&session_id_str).await.ok_or_else(|| {
            error!("Session not found: {}", session_id_str);
            acp::Error::invalid_params()
        })?;
        validate_prompt_support(&model, &content)?;

        let dispatch = self.registry.begin_prompt(&session_id_str).await.ok_or_else(|| {
            error!("Session not found: {}", session_id_str);
            acp::Error::invalid_params()
        })?;

        let (result_tx, result_rx) = oneshot::channel();
        dispatch
            .relay_tx
            .send(SessionCommand::Prompt {
                content,
                switch_model: dispatch.switch_model,
                reasoning_effort: dispatch.reasoning_effort,
                result_tx,
            })
            .await
            .map_err(|_| {
                error!("Relay channel closed for session {}", session_id_str);
                acp::Error::internal_error()
            })?;

        let stop_reason = result_rx
            .await
            .map_err(|_| {
                error!("Relay dropped result channel for session {}", session_id_str);
                acp::Error::internal_error()
            })?
            .map_err(|e| {
                error!("Relay error for session {}: {}", session_id_str, e);
                acp::Error::internal_error()
            })?;

        info!("Prompt completed with stop reason: {:?}", stop_reason);
        Ok(PromptResponse::new(stop_reason))
    }

    pub async fn cancel(&self, args: acp::CancelNotification) -> Result<(), acp::Error> {
        info!("Received cancel for session: {:?}", args.session_id);
        let session_id_str = args.session_id.0.to_string();
        let relay = self.registry.relay(&session_id_str).await.ok_or_else(|| {
            error!("Session not found for cancel: {}", session_id_str);
            acp::Error::invalid_params()
        })?;

        relay.cmd.send(SessionCommand::Cancel).await.map_err(|_| {
            error!("Relay channel closed for cancel: {}", session_id_str);
            acp::Error::internal_error()
        })?;

        Ok(())
    }

    pub async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, acp::Error> {
        let session_id_str = args.session_id.0.to_string();
        let config_id = args.config_id.0.to_string();
        let value = args.value.0.to_string();

        info!("set_session_config_option: session={}, config={}, value={}", session_id_str, config_id, value);

        let setting = ConfigSetting::parse(&config_id, &value).map_err(|e| {
            error!("{e}");
            acp::Error::invalid_params()
        })?;

        let available = get_local_models().await;
        let all_models = get_all_models(&available);

        let snapshot =
            self.registry.apply_config_change(&session_id_str, &setting, &available).await.ok_or_else(|| {
                error!("Session not found: {}", session_id_str);
                acp::Error::invalid_params()
            })??;

        let options = options_from_snapshot(&snapshot, &available, &all_models, &OAuthCredentialStore::default());
        Ok(SetSessionConfigOptionResponse::new(options))
    }

    pub async fn on_mcp_request(&self, request: McpRequest) -> Result<(), acp::Error> {
        info!("Received MCP ext request: {:?}", request);
        match request {
            McpRequest::Authenticate { session_id, server_name } => {
                let relay = self.registry.relay(&session_id).await.ok_or_else(|| {
                    error!("Session not found for authenticate_mcp_server: {}", session_id);
                    acp::Error::invalid_params()
                })?;

                relay.mcp_request.send(McpRequest::Authenticate { session_id, server_name }).await.map_err(|_| {
                    error!("MCP request channel closed for session");
                    acp::Error::internal_error()
                })?;
            }
        }

        Ok(())
    }
}
