use aether_auth::OAuthCredentialStorage;
use aether_core::agent_spec::AgentSpec;
use aether_core::core::AgentDeps;
use aether_core::events::DynObserverFactory;
use aether_project::AgentCatalog;
use aether_sessions::model::{SessionEvent, SessionMeta, last_agent_from_events};
use agent_client_protocol::schema::v1::{self as acp, LoadSessionRequest, NewSessionRequest, SessionId};
use agent_client_protocol::{Client, ConnectionTo};
use llm::catalog::{LlmModel, get_local_models};
use llm::types::IsoString;
use llm::{ProviderConnectionOverrides, ReasoningEffort};
use rmcp::model::ClientCapabilities;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info, warn};

use super::agent_key::AgentKey;
use super::agent_runtime::{ProductionRuntimeFactory, RuntimeFactory};
use super::model_config::{Modes, pick_default_model};
use super::protocol::mcp::map_acp_mcp_servers;
use super::session_actor::{SessionActor, SessionActorInit, SessionHandle};
use super::session_agents::SessionAgents;
use super::session_config_state::SessionConfigState;
use crate::resolve::{InitialSessionSelection, resolve_agent_from_catalog};
use crate::settings_args::SettingsSourceArgs;
use aether_sessions::SessionStore;

/// Builds the per-session actor for both new and loaded sessions, resolving
/// settings, agent catalog, model discovery, and the runtime factory.
pub(crate) struct SessionFactory {
    settings_source: SettingsSourceArgs,
    provider_connections: ProviderConnectionOverrides,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    session_store: Arc<SessionStore>,
    initial_selection: InitialSessionSelection,
    observer_factory: Option<DynObserverFactory>,
}

/// The fully-built session ready to be registered with [`AcpState`](super::state::AcpState).
pub(crate) struct CreatedSession {
    pub session_id: SessionId,
    pub handle: SessionHandle,
    pub config_options: Vec<acp::SessionConfigOption>,
    pub replay_events: Vec<SessionEvent>,
}

impl SessionFactory {
    pub(crate) fn new(
        settings_source: SettingsSourceArgs,
        provider_connections: ProviderConnectionOverrides,
        oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
        session_store: Arc<SessionStore>,
        initial_selection: InitialSessionSelection,
        observer_factory: Option<DynObserverFactory>,
    ) -> Self {
        Self {
            settings_source,
            provider_connections,
            oauth_credential_store,
            session_store,
            initial_selection,
            observer_factory,
        }
    }

    pub(crate) async fn create(
        &self,
        mut args: NewSessionRequest,
        cx: &ConnectionTo<Client>,
        mcp_capabilities: ClientCapabilities,
    ) -> Result<CreatedSession, acp::Error> {
        // Inside a sandbox container the client sends the *host* cwd, but the
        // project is mounted at the container's working directory.
        if std::env::var("AETHER_INSIDE_SANDBOX").is_ok() {
            let container_cwd = std::env::current_dir().unwrap_or_else(|_| "/workspace".into());
            info!("Sandbox: remapping cwd {:?} -> {:?}", args.cwd, container_cwd);
            args.cwd = container_cwd;
        }

        info!("Creating new session with cwd: {:?}", args.cwd);
        let session_id = uuid::Uuid::new_v4().to_string();

        let mut mode_catalog = self.load_mode_catalog(&args.cwd).await?;
        let default_model = pick_default_model(&mode_catalog.available).cloned().ok_or_else(|| {
            error!("No models available — set an API key env var (e.g. ANTHROPIC_API_KEY)");
            acp::Error::internal_error()
        })?;
        let resolved = self.resolve_new_session(&mut mode_catalog, &default_model)?;

        let meta = SessionMeta {
            session_id: session_id.clone(),
            cwd: args.cwd.clone(),
            model: resolved.config.active_model.clone(),
            selected_mode: resolved.config.selected_mode.clone(),
            created_at: IsoString::now().0,
        };
        if let Err(e) = self.session_store.append_meta(&session_id, &meta) {
            error!("Failed to write session meta: {e}");
        }

        let runtime_factory =
            self.production_runtime_factory(args.cwd, args.mcp_servers, mode_catalog.specs.catalog(), mcp_capabilities);
        self.build_session(SessionId::new(session_id), runtime_factory, mode_catalog, resolved, Vec::new(), cx).await
    }

    pub(crate) async fn load(
        &self,
        args: LoadSessionRequest,
        cx: &ConnectionTo<Client>,
        mcp_capabilities: ClientCapabilities,
    ) -> Result<CreatedSession, acp::Error> {
        let session_id = args.session_id.0.to_string();
        info!("Loading session: {session_id}");

        let (meta, events) = self.session_store.load(&session_id).ok_or_else(|| {
            error!("Session not found: {session_id}");
            acp::Error::invalid_params()
        })?;

        let mut mode_catalog = self.load_mode_catalog(&args.cwd).await?;
        let resolved = resolve_loaded_session(&mut mode_catalog, &meta, &events)?;

        let runtime_factory =
            self.production_runtime_factory(args.cwd, args.mcp_servers, mode_catalog.specs.catalog(), mcp_capabilities);
        self.build_session(SessionId::new(session_id), runtime_factory, mode_catalog, resolved, events, cx).await
    }

    fn production_runtime_factory(
        &self,
        cwd: PathBuf,
        mcp_servers: Vec<acp::McpServer>,
        catalog: &AgentCatalog,
        mcp_capabilities: ClientCapabilities,
    ) -> Arc<dyn RuntimeFactory> {
        let deps = AgentDeps::new(Arc::clone(&self.oauth_credential_store), self.observer_factory.clone())
            .with_agent_registry(catalog.registry().clone())
            .with_mcp_client_capabilities(mcp_capabilities);
        Arc::new(ProductionRuntimeFactory::new(cwd, map_acp_mcp_servers(mcp_servers), deps))
    }

    async fn build_session(
        &self,
        session_id: SessionId,
        runtime_factory: Arc<dyn RuntimeFactory>,
        mode_catalog: SessionModeCatalog,
        resolved: ResolvedSession,
        transcript: Vec<SessionEvent>,
        cx: &ConnectionTo<Client>,
    ) -> Result<CreatedSession, acp::Error> {
        let handle = SessionActor::spawn(SessionActorInit {
            session_id: session_id.clone(),
            connection: cx.clone(),
            repository: self.session_store.clone(),
            oauth_credential_store: Arc::clone(&self.oauth_credential_store),
            active_agent: resolved.active_agent,
            specs: mode_catalog.specs,
            runtime_factory,
            transcript: transcript.clone(),
            modes: mode_catalog.modes,
            config: resolved.config,
        })
        .await
        .map_err(|e| {
            error!("Failed to start session actor: {e}");
            acp::Error::internal_error()
        })?;

        let config_options =
            handle.config_snapshot().config_options(&mode_catalog.available, self.oauth_credential_store.as_ref());

        info!("Session {} ready", session_id.0);
        Ok(CreatedSession { session_id, handle, config_options, replay_events: transcript })
    }

    fn resolve_new_session(
        &self,
        mode_catalog: &mut SessionModeCatalog,
        default_model: &LlmModel,
    ) -> Result<ResolvedSession, acp::Error> {
        let selection = match &self.initial_selection {
            InitialSessionSelection::Agent(agent) => {
                if !mode_catalog.modes.iter().any(|mode| mode.name == *agent) {
                    warn!("Unknown or unavailable agent `{agent}` requested via --agent");
                    return Err(acp::Error::invalid_params());
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
                acp::Error::invalid_params()
            })?;

        if selected.spec.name == "__default__" {
            Ok(resolve_model_spec_session(mode_catalog, selected.spec))
        } else {
            if !mode_catalog.modes.iter().any(|mode| mode.name == selected.spec.name) {
                warn!("Configured default agent `{}` is unavailable", selected.spec.name);
                return Err(acp::Error::invalid_params());
            }
            resolve_named_session(mode_catalog, &selected.spec.name)
        }
    }

    async fn load_mode_catalog(&self, cwd: &Path) -> Result<SessionModeCatalog, acp::Error> {
        let catalog = self
            .settings_source
            .load_agent_catalog(cwd)
            .map_err(|e| {
                error!("Failed to load agent catalog: {e}");
                acp::Error::invalid_params()
            })?
            .with_provider_connections(self.provider_connections.clone());

        let available = get_local_models().await;
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
) -> Result<ResolvedSession, acp::Error> {
    if let Some(name) = last_agent_from_events(meta.selected_mode.clone(), events).as_deref() {
        return resolve_named_session(mode_catalog, name);
    }

    let parsed_model: LlmModel = meta.model.parse().map_err(|e: String| {
        error!("Failed to parse restored model '{}': {e}", meta.model);
        acp::Error::invalid_params()
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

fn resolve_named_session(mode_catalog: &SessionModeCatalog, name: &str) -> Result<ResolvedSession, acp::Error> {
    let spec = mode_catalog.specs.get(&AgentKey::Named(name.to_owned())).ok_or_else(|| {
        error!("Failed to resolve runtime inputs for mode '{name}'");
        acp::Error::invalid_params()
    })?;
    let config = SessionConfigState::with_selection(spec.model.clone(), Some(name.to_string()), spec.reasoning_effort);
    Ok(ResolvedSession { active_agent: AgentKey::Named(name.to_string()), config })
}

fn parse_available_model(model: &str, available: &[LlmModel]) -> Result<LlmModel, acp::Error> {
    let parsed = model.parse().map_err(|e: String| {
        warn!("Failed to parse --model `{model}`: {e}");
        acp::Error::invalid_params()
    })?;

    if available.contains(&parsed) {
        Ok(parsed)
    } else {
        warn!("Requested model `{model}` is not available");
        Err(acp::Error::invalid_params())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONNET: &str = "anthropic:claude-sonnet-4-5";
    const DEEPSEEK: &str = "deepseek:deepseek-chat";
    const BEDROCK_ARN_AS_MODEL_REJECTED: &str =
        "bedrock:arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000";

    #[test]
    fn parse_available_model_rejects_bedrock_inference_profile_arn() {
        let available: Vec<LlmModel> = Vec::new();
        let error = parse_available_model(BEDROCK_ARN_AS_MODEL_REJECTED, &available).unwrap_err();
        assert_eq!(error, acp::Error::invalid_params());
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
