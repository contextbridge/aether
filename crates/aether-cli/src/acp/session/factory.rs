use aether_auth::OAuthCredentialStorage;
use aether_core::agent_spec::AgentSpec;
use aether_core::core::AgentDeps;
use aether_core::events::DynObserverFactory;
use aether_project::AgentCatalog;
use aether_sessions::model::{SessionEvent, SessionMeta, last_agent_from_events};
use agent_client_protocol::schema::v2::{self as acp, NewSessionRequest, ResumeSessionRequest, SessionId};
use agent_client_protocol::{Client, ConnectionTo, Error};
use llm::catalog::{LlmModel, get_local_models};
use llm::types::IsoString;
use llm::{ProviderConnectionOverrides, ReasoningEffort};
use rmcp::model::ClientCapabilities;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::{error, info, warn};

use super::actor::{SessionActor, SessionActorInit, SessionHandle};
use super::agent_key::AgentKey;
use super::agents::SessionAgents;
use super::config::SessionConfigState;
use super::model::{Modes, pick_default_model};
use super::runtime::{ProductionRuntimeFactory, RuntimeFactory};
use crate::acp::protocol::mcp::map_acp_mcp_servers;
use crate::resolve::{InitialSessionSelection, resolve_agent_from_catalog};
use crate::settings_args::SettingsSourceArgs;
use aether_sessions::{SessionStore, SessionStoreError};

/// Builds the per-session actor for both new and resumed sessions, resolving
/// settings, agent catalog, model discovery, and the runtime factory.
pub(crate) struct SessionFactory {
    settings_source: SettingsSourceArgs,
    provider_connections: ProviderConnectionOverrides,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    session_store: Arc<SessionStore>,
    initial_selection: InitialSessionSelection,
    observer_factory: Option<DynObserverFactory>,
    runtime_factory: Option<Arc<dyn RuntimeFactory>>,
    available: OnceCell<Vec<LlmModel>>,
}

/// The fully-built session ready to be registered with [`AcpState`](crate::acp::state::AcpState).
pub(crate) struct CreatedSession {
    pub session_id: SessionId,
    pub handle: SessionHandle,
    pub config_options: Vec<acp::SessionConfigOption>,
}

pub(crate) struct PreparedSession {
    init: SessionActorInit,
    available: Vec<LlmModel>,
}

impl PreparedSession {
    pub(crate) fn refresh_transcript(mut self) -> Result<Self, Error> {
        let (meta, transcript) =
            self.init.repository.load(&self.init.session_id.0).map_err(Error::into_internal_error)?;
        let mut catalog =
            SessionModeCatalog { specs: self.init.specs, modes: self.init.modes, available: self.available };
        let resolved = resolve_loaded_session(&mut catalog, &meta, &transcript)?;
        self.init.specs = catalog.specs;
        self.init.modes = catalog.modes;
        self.available = catalog.available;
        self.init.active_agent = resolved.active_agent;
        self.init.config = resolved.config;
        self.init.transcript = transcript;
        Ok(self)
    }

    pub(crate) async fn start(self) -> Result<CreatedSession, Error> {
        let session_id = self.init.session_id.clone();
        let config_options = self.init.config.config_options(
            &self.init.modes,
            &self.available,
            self.init.oauth_credential_store.as_ref(),
        );
        let handle = SessionActor::spawn(self.init).await.map_err(|e| {
            error!("Failed to start session actor: {e}");
            Error::internal_error()
        })?;
        Ok(CreatedSession { session_id, handle, config_options })
    }
}

impl SessionFactory {
    pub(crate) fn new(
        settings_source: SettingsSourceArgs,
        provider_connections: ProviderConnectionOverrides,
        oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
        session_store: Arc<SessionStore>,
        initial_selection: InitialSessionSelection,
        observer_factory: Option<DynObserverFactory>,
        runtime_factory: Option<Arc<dyn RuntimeFactory>>,
    ) -> Self {
        Self {
            settings_source,
            provider_connections,
            oauth_credential_store,
            session_store,
            initial_selection,
            observer_factory,
            runtime_factory,
            available: OnceCell::new(),
        }
    }

    pub(crate) async fn available_models(&self) -> &[LlmModel] {
        self.available.get_or_init(get_local_models).await
    }

    pub(crate) async fn prepare_new(
        &self,
        mut args: NewSessionRequest,
        cx: &ConnectionTo<Client>,
        mcp_capabilities: ClientCapabilities,
    ) -> Result<PreparedSession, Error> {
        // Inside a sandbox container the client sends the *host* cwd, but the
        // project is mounted at the container's working directory.
        if std::env::var("AETHER_INSIDE_SANDBOX").is_ok() {
            let container_cwd = std::env::current_dir().unwrap_or_else(|_| "/workspace".into());
            info!("Sandbox: remapping cwd {:?} -> {:?}", args.cwd, container_cwd);
            args.cwd = acp::AbsolutePath::new(container_cwd);
        }

        info!("Creating new session with cwd: {:?}", args.cwd);
        let session_id = uuid::Uuid::new_v4().to_string();

        let mut mode_catalog = self.load_mode_catalog(args.cwd.as_ref()).await?;
        let default_model = pick_default_model(&mode_catalog.available).cloned().ok_or_else(|| {
            error!("No models available — set an API key env var (e.g. ANTHROPIC_API_KEY)");
            Error::internal_error()
        })?;
        let resolved = self.resolve_new_session(&mut mode_catalog, &default_model)?;

        let meta = SessionMeta {
            session_id: session_id.clone(),
            cwd: args.cwd.clone().into_inner(),
            model: resolved.config.active_model.clone(),
            selected_mode: resolved.config.selected_mode.clone(),
            created_at: IsoString::now().0,
        };
        if let Err(e) = self.session_store.append_meta(&session_id, &meta) {
            error!("Failed to write session meta: {e}");
        }

        let runtime_factory = self.runtime_factory.clone().unwrap_or_else(|| {
            self.production_runtime_factory(
                args.cwd.into_inner(),
                args.mcp_servers,
                mode_catalog.specs.catalog(),
                mcp_capabilities,
                &session_id,
            )
        });
        Ok(self.prepare_session(
            SessionId::new(session_id),
            runtime_factory,
            mode_catalog,
            resolved,
            Vec::new(),
            false,
            cx,
        ))
    }

    pub(crate) async fn prepare_resume(
        &self,
        args: ResumeSessionRequest,
        cx: &ConnectionTo<Client>,
        mcp_capabilities: ClientCapabilities,
    ) -> Result<PreparedSession, Error> {
        let replay = match args.replay_from {
            Some(acp::ReplayFrom::Start(_)) => true,
            None => false,
            Some(_) => return Err(Error::invalid_params()),
        };
        self.restore(args.session_id, args.cwd.into_inner(), args.mcp_servers, cx, mcp_capabilities, replay).await
    }

    async fn restore(
        &self,
        session_id: SessionId,
        cwd: PathBuf,
        mcp_servers: Vec<acp::McpServer>,
        cx: &ConnectionTo<Client>,
        mcp_capabilities: ClientCapabilities,
        replay: bool,
    ) -> Result<PreparedSession, Error> {
        let session_id_string = session_id.0.to_string();
        info!("Restoring session: {session_id_string}");

        let (meta, events) = self.session_store.load(&session_id_string).map_err(|error| match error {
            SessionStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                error!("Session not found: {session_id_string}");
                Error::invalid_params()
            }
            error => {
                error!("Failed to load session {session_id_string}: {error}");
                Error::internal_error()
            }
        })?;

        let mut mode_catalog = self.load_mode_catalog(&cwd).await?;
        let resolved = resolve_loaded_session(&mut mode_catalog, &meta, &events)?;
        let runtime_factory = self.runtime_factory.clone().unwrap_or_else(|| {
            self.production_runtime_factory(
                cwd,
                mcp_servers,
                mode_catalog.specs.catalog(),
                mcp_capabilities,
                session_id.0.as_ref(),
            )
        });
        Ok(self.prepare_session(session_id, runtime_factory, mode_catalog, resolved, events, replay, cx))
    }

    fn production_runtime_factory(
        &self,
        cwd: PathBuf,
        mcp_servers: Vec<acp::McpServer>,
        catalog: &AgentCatalog,
        mcp_capabilities: ClientCapabilities,
        session_affinity_key: &str,
    ) -> Arc<dyn RuntimeFactory> {
        let deps = AgentDeps::new(Arc::clone(&self.oauth_credential_store), self.observer_factory.clone())
            .with_agent_registry(catalog.registry().clone())
            .with_mcp_client_capabilities(mcp_capabilities)
            .with_session_affinity_key(session_affinity_key);
        Arc::new(ProductionRuntimeFactory::new(cwd, map_acp_mcp_servers(mcp_servers), deps))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_session(
        &self,
        session_id: SessionId,
        runtime_factory: Arc<dyn RuntimeFactory>,
        mode_catalog: SessionModeCatalog,
        resolved: ResolvedSession,
        transcript: Vec<SessionEvent>,
        replay: bool,
        cx: &ConnectionTo<Client>,
    ) -> PreparedSession {
        let init = SessionActorInit {
            session_id,
            connection: cx.clone(),
            repository: self.session_store.clone(),
            oauth_credential_store: Arc::clone(&self.oauth_credential_store),
            active_agent: resolved.active_agent,
            specs: mode_catalog.specs,
            runtime_factory,
            transcript,
            replay,
            modes: mode_catalog.modes,
            config: resolved.config,
        };
        PreparedSession { init, available: mode_catalog.available }
    }

    fn resolve_new_session(
        &self,
        mode_catalog: &mut SessionModeCatalog,
        default_model: &LlmModel,
    ) -> Result<ResolvedSession, Error> {
        let selection = match &self.initial_selection {
            InitialSessionSelection::Agent(agent) => {
                if !mode_catalog.modes.iter().any(|mode| mode.name == *agent) {
                    warn!("Unknown or unavailable agent `{agent}` requested via --agent");
                    return Err(Error::invalid_params());
                }
                self.initial_selection.clone()
            }
            InitialSessionSelection::Model { model, reasoning_effort } => {
                let model = parse_available_model(model, &mode_catalog.available)?;
                InitialSessionSelection::Model { model: model.to_string(), reasoning_effort: *reasoning_effort }
            }
            InitialSessionSelection::Default if mode_catalog.specs.catalog().default_agent().is_none() => {
                InitialSessionSelection::Model { model: default_model.to_string(), reasoning_effort: None }
            }
            InitialSessionSelection::Default => self.initial_selection.clone(),
        };

        let selected =
            resolve_agent_from_catalog(mode_catalog.specs.catalog().clone(), &selection).map_err(|error| {
                warn!("Failed to resolve initial agent: {error}");
                Error::invalid_params()
            })?;

        if selected.spec.name == "__default__" {
            Ok(resolve_model_spec_session(mode_catalog, selected.spec))
        } else {
            if !mode_catalog.modes.iter().any(|mode| mode.name == selected.spec.name) {
                warn!("Configured default agent `{}` is unavailable", selected.spec.name);
                return Err(Error::invalid_params());
            }
            resolve_named_session(mode_catalog, &selected.spec.name)
        }
    }

    async fn load_mode_catalog(&self, cwd: &Path) -> Result<SessionModeCatalog, Error> {
        let catalog = self
            .settings_source
            .load_agent_catalog(cwd)
            .map_err(|e| {
                error!("Failed to load agent catalog: {e}");
                Error::invalid_params()
            })?
            .with_provider_connections(self.provider_connections.clone());

        let available = self.available_models().await.to_vec();
        let modes = Modes::from_specs(catalog.all(), &available);

        Ok(SessionModeCatalog { specs: SessionAgents::new(catalog), modes, available })
    }
}

struct SessionModeCatalog {
    specs: SessionAgents,
    modes: Modes,
    available: Vec<LlmModel>,
}

struct ResolvedSession {
    active_agent: AgentKey,
    config: SessionConfigState,
}

fn resolve_loaded_session(
    mode_catalog: &mut SessionModeCatalog,
    meta: &SessionMeta,
    events: &[SessionEvent],
) -> Result<ResolvedSession, Error> {
    if let Some(name) = last_agent_from_events(meta.selected_mode.clone(), events).as_deref() {
        return resolve_named_session(mode_catalog, name);
    }

    let parsed_model: LlmModel = meta.model.parse().map_err(|e: String| {
        error!("Failed to parse restored model '{}': {e}", meta.model);
        Error::invalid_params()
    })?;
    Ok(resolve_model_session(mode_catalog, &parsed_model, None))
}

fn resolve_model_spec_session(mode_catalog: &mut SessionModeCatalog, spec: AgentSpec) -> ResolvedSession {
    let config = SessionConfigState::with_selection(spec.model.clone(), None, spec.reasoning_effort);
    mode_catalog.specs.set_default(spec);
    ResolvedSession { active_agent: AgentKey::Default, config }
}

fn resolve_model_session(
    mode_catalog: &mut SessionModeCatalog,
    model: &LlmModel,
    reasoning_effort: Option<ReasoningEffort>,
) -> ResolvedSession {
    let spec = mode_catalog.specs.catalog().default_spec(model, reasoning_effort);
    resolve_model_spec_session(mode_catalog, spec)
}

fn resolve_named_session(mode_catalog: &SessionModeCatalog, name: &str) -> Result<ResolvedSession, Error> {
    let spec = mode_catalog.specs.get(&AgentKey::Named(name.to_owned())).ok_or_else(|| {
        error!("Failed to resolve runtime inputs for mode '{name}'");
        Error::invalid_params()
    })?;
    let config = SessionConfigState::with_selection(spec.model.clone(), Some(name.to_string()), spec.reasoning_effort);
    Ok(ResolvedSession { active_agent: AgentKey::Named(name.to_string()), config })
}

fn parse_available_model(model: &str, available: &[LlmModel]) -> Result<LlmModel, Error> {
    let parsed = model.parse().map_err(|e: String| {
        warn!("Failed to parse --model `{model}`: {e}");
        Error::invalid_params()
    })?;

    if available.contains(&parsed) {
        Ok(parsed)
    } else {
        warn!("Requested model `{model}` is not available");
        Err(Error::invalid_params())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONNET: &str = "anthropic:claude-sonnet-4-5";
    const DEEPSEEK: &str = "deepseek:deepseek-v4-flash";
    const BEDROCK_ARN_AS_MODEL_REJECTED: &str =
        "bedrock:arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000";

    #[test]
    fn parse_available_model_rejects_bedrock_inference_profile_arn() {
        let available: Vec<LlmModel> = Vec::new();
        let error = parse_available_model(BEDROCK_ARN_AS_MODEL_REJECTED, &available).unwrap_err();
        assert_eq!(error, Error::invalid_params());
    }

    #[test]
    fn parse_available_model_rejects_unknown_catalog_model() {
        let available: Vec<LlmModel> = vec![DEEPSEEK.parse().unwrap()];
        assert!(parse_available_model(SONNET, &available).is_err());
    }

    #[test]
    fn parse_available_model_accepts_catalog_model_when_present() {
        let sonnet: LlmModel = SONNET.parse().unwrap();
        let available = vec![sonnet.clone()];
        assert_eq!(parse_available_model(SONNET, &available).unwrap(), sonnet);
    }
}
