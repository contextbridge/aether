use super::agent::acp_agent_builder;
use super::agent_key::AgentKey;
use super::agent_runtime::{AgentRuntime, RuntimeEvent, RuntimeFactory};
use super::error::SessionError;
use super::fake_prompt_mcp::FakePromptMcp;
use super::model_config::{Modes, ValidatedMode};
use super::session_actor::{SessionActor, SessionActorInit};
use super::session_agents::SessionAgents;
use super::session_config_state::SessionConfigState;
use super::state::{AcpState, AcpStateConfig};
use crate::acp::session_store::SessionStore;
use crate::error::CliError;
use crate::resolve::InitialSessionSelection;
use crate::settings_args::SettingsSourceArgs;
use crate::workspace::WorkspaceManager;
use crate::workspace::testing::StdCopyCloner;
use acp_utils::notifications::McpNotification;
use acp_utils::testing::{TestPeer, duplex_pair};
use aether_auth::OAuthCredentialStorage;
use aether_core::agent_spec::{AgentSpec, AgentSpecExposure};
use aether_core::core::{AgentBuilder, AgentHandle, Prompt};
use aether_core::events::{AgentEvent, Command, MessageEvent};
use aether_core::mcp::{ServerFactory, mcp};
use aether_project::AgentCatalog;
use aether_sessions::{SessionControlEvent, SessionEvent, SessionMeta, UserEvent, last_agent_from_events};
use agent_client_protocol::schema::v1::{SessionId, SessionUpdate};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use futures::FutureExt;
use llm::ProviderConnectionOverrides;
use llm::testing::FakeLlmProvider;
use llm::{ChatMessage, Context, LlmResponse, StreamingModelProvider};
use mcp_utils::client::{InMemoryServerSpec, McpServer, McpTransport, ToolExposure};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::spawn_local;

const PLANNER_REPLY: &str = "planner reply";
const CODER_REPLY: &str = "coder reply";

/// In-memory ACP harness running the real `acp_agent_builder` against a
/// pre-wired test client. Created via [`AcpTestHarness::start`] inside a
/// `LocalSet`. The harness owns an [`AcpState`] and a temp-dir-backed
/// [`SessionStore`] so tests can register fake-driven sessions without
/// going through `new_session`.
pub struct AcpTestHarness {
    pub client_cx: ConnectionTo<Agent>,
    pub peer: TestPeer,
    agent_cx: ConnectionTo<Client>,
    state: Arc<AcpState>,
    session_store: Arc<SessionStore>,
    _tmp: tempfile::TempDir,
}

pub struct FakeAgentSwitchingSession {
    session_id: SessionId,
    planner: FakeAcpAgent,
    coder: FakeAcpAgent,
}

#[derive(Clone)]
pub struct FakeAcpAgent {
    name: String,
    captured_contexts: Arc<Mutex<Vec<Context>>>,
}

impl AcpTestHarness {
    pub async fn start() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir for session store");
        let session_store = Arc::new(SessionStore::from_path(tmp.path().to_path_buf()));
        let workspace_manager = Arc::new(WorkspaceManager::from_registry_path_with_cloner(
            tmp.path().join("workspaces.json"),
            Arc::new(StdCopyCloner),
        ));
        let state = Arc::new(AcpState::new(AcpStateConfig {
            session_store: session_store.clone(),
            workspace_manager,
            oauth_credential_store: fake_oauth_store(),
            initial_selection: InitialSessionSelection::default(),
            settings_source: SettingsSourceArgs::default(),
            provider_connections: ProviderConnectionOverrides::default(),
            telemetry: None,
        }));

        let (peer, client_builder) = TestPeer::new();
        let (agent_transport, client_transport) = duplex_pair();
        let (agent_cx_tx, agent_cx_rx) = oneshot::channel::<ConnectionTo<Client>>();
        let (client_cx_tx, client_cx_rx) = oneshot::channel::<ConnectionTo<Agent>>();

        let server_state = state.clone();
        spawn_local(async move {
            let _ = acp_agent_builder(server_state)
                .connect_with(agent_transport, async move |cx: ConnectionTo<Client>| {
                    let _ = agent_cx_tx.send(cx);
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await;
        });

        spawn_local(async move {
            let _ = client_builder
                .connect_with(client_transport, async move |cx: ConnectionTo<Agent>| {
                    let _ = client_cx_tx.send(cx);
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await;
        });

        let agent_cx = agent_cx_rx.await.expect("agent side connect_with produced a ConnectionTo");
        let client_cx = client_cx_rx.await.expect("client side connect_with produced a ConnectionTo");
        Self { client_cx, peer, agent_cx, state, session_store, _tmp: tmp }
    }

    pub async fn insert_agent_switching_session(&self) -> FakeAgentSwitchingSession {
        self.insert_switching_session(
            SessionId::new("agent-switching-session"),
            Vec::new(),
            Some("Planner".to_string()),
            false,
        )
        .await
    }

    pub async fn insert_agent_switching_session_with_serverless_coder(&self) -> FakeAgentSwitchingSession {
        self.insert_switching_session(
            SessionId::new("agent-switching-serverless-session"),
            Vec::new(),
            Some("Planner".to_string()),
            true,
        )
        .await
    }

    pub async fn insert_loaded_agent_switching_session(&self, session_id: &str) -> FakeAgentSwitchingSession {
        let events = self.session_store.load(session_id).map(|(_, events)| events).unwrap_or_default();
        let selected_mode = last_agent_from_events(Some("Planner".to_string()), &events);
        self.insert_switching_session(SessionId::new(session_id), events, selected_mode, false).await
    }

    pub async fn expect_mcp_server_status(&mut self, expected: &[&str]) {
        assert_server_status(self.peer.next_mcp_notification().await, expected);
    }

    pub async fn expect_mcp_server_status_exact(&mut self, expected: &[&str]) {
        assert_server_status_exact(self.peer.next_mcp_notification().await, expected);
    }

    pub async fn expect_available_commands(&mut self, expected: &[&str], unexpected: &[&str]) {
        loop {
            let update = self.peer.next_session_notification().await.update;
            if matches!(update, SessionUpdate::AvailableCommandsUpdate(_)) {
                assert_available_commands(update, expected, unexpected);
                return;
            }
        }
    }

    pub fn append_agent_switch(&self, session_id: &str, from: Option<&str>, to: Option<&str>) {
        self.append_stored_event(
            session_id,
            &SessionEvent::Control(SessionControlEvent::AgentSwitched {
                from: from.map(str::to_string),
                to: to.map(str::to_string),
            }),
        );
    }

    /// Register a stub session built from a hand-spawned
    /// `(agent_tx, agent_rx, agent_handle)` triple — typically from
    /// `aether_core::core::agent(fake_llm).spawn().await`. Pairs the agent with a
    /// real but empty in-memory MCP (no servers). The session is routable via
    /// `state.route_prompt(id)` / `state.cancel(id)`.
    pub async fn insert_stub_session(
        &self,
        agent_tx: mpsc::Sender<Command>,
        agent_rx: mpsc::Receiver<AgentEvent>,
        agent_handle: AgentHandle,
        id: SessionId,
        model: &str,
    ) {
        let model_spec: llm::catalog::LlmModel = "anthropic:claude-sonnet-4-5".parse().expect("test model parses");
        let mut specs = SessionAgents::new(AgentCatalog::empty(PathBuf::from("/tmp")));
        specs.set_default(AgentSpec::bare(&model_spec, None, Vec::new()));
        let factory = Arc::new(StubRuntimeFactory {
            cwd: PathBuf::from("/tmp"),
            agent_parts: Mutex::new(Some(StubAgentParts { tx: agent_tx, rx: agent_rx, handle: agent_handle })),
        });

        let handle = SessionActor::spawn(SessionActorInit {
            session_id: id.clone(),
            connection: self.agent_cx.clone(),
            repository: self.session_store.clone(),
            oauth_credential_store: fake_oauth_store(),
            active_agent: AgentKey::Default,
            specs,
            runtime_factory: factory,
            transcript: Vec::new(),
            modes: Modes::default(),
            config: SessionConfigState::with_selection(model.to_string(), None, None),
        })
        .await
        .expect("stub session actor spawns");
        self.state.register_session(&id, handle).await;
    }

    pub fn append_stored_session(&self, session_id: &str, created_at: &str) {
        self.append_stored_session_in(session_id, created_at, std::path::Path::new("/tmp"));
    }

    pub fn append_stored_session_in(&self, session_id: &str, created_at: &str, cwd: &std::path::Path) {
        let meta = SessionMeta {
            session_id: session_id.to_string(),
            cwd: cwd.to_path_buf(),
            model: "test-model".to_string(),
            selected_mode: None,
            created_at: created_at.to_string(),
        };

        self.session_store.append_meta(session_id, &meta).expect("stored session meta appends");
    }

    pub fn append_stored_prompt(&self, session_id: &str, prompt: &str) {
        self.append_stored_event(
            session_id,
            &SessionEvent::User(UserEvent::Message { content: vec![llm::ContentBlock::text(prompt)] }),
        );
    }

    pub fn append_stored_user_blocks(&self, session_id: &str, blocks: Vec<llm::ContentBlock>) {
        self.append_stored_event(session_id, &SessionEvent::User(UserEvent::Message { content: blocks }));
    }

    pub fn append_stored_agent_text(&self, session_id: &str, text: &str) {
        self.append_stored_event(
            session_id,
            &SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
                message_id: "msg".to_string(),
                chunk: text.to_string(),
                is_complete: true,
            })),
        );
    }

    async fn insert_switching_session(
        &self,
        acp_session_id: SessionId,
        events: Vec<SessionEvent>,
        selected_mode: Option<String>,
        serverless_coder: bool,
    ) -> FakeAgentSwitchingSession {
        let (planner_def, planner) = fake_agent("Planner", "planner-mcp", "plan", PLANNER_REPLY);
        let (mut coder_def, coder) = fake_agent("Coder", "coder-mcp", "edit", CODER_REPLY);
        if serverless_coder {
            coder_def.mcp = None;
        }

        let mut catalog_specs = Vec::new();
        let mut agents = HashMap::new();
        for def in [planner_def, coder_def] {
            catalog_specs.push(def.spec.clone());
            agents.insert(def.spec.name.clone(), def);
        }
        let specs = SessionAgents::new(AgentCatalog::new(PathBuf::from("/tmp"), catalog_specs, None));

        let factory = Arc::new(FakeRuntimeFactory { cwd: PathBuf::from("/tmp"), agents });
        let initial_agent = selected_mode.clone().unwrap_or_else(|| "Planner".to_string());

        let handle = SessionActor::spawn(SessionActorInit {
            session_id: acp_session_id.clone(),
            connection: self.agent_cx.clone(),
            repository: self.session_store.clone(),
            oauth_credential_store: fake_oauth_store(),
            active_agent: AgentKey::Named(initial_agent),
            specs,
            runtime_factory: factory,
            transcript: events,
            modes: switching_modes(),
            config: SessionConfigState::with_selection("anthropic:claude-sonnet-4-5".to_string(), selected_mode, None),
        })
        .await
        .expect("fake agent switching session actor spawns");
        self.state.register_session(&acp_session_id, handle).await;
        FakeAgentSwitchingSession { session_id: acp_session_id, planner, coder }
    }

    fn append_stored_event(&self, session_id: &str, event: &SessionEvent) {
        self.session_store.append_event(session_id, event).expect("stored session event appends");
    }
}

impl FakeAgentSwitchingSession {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn planner(&self) -> &FakeAcpAgent {
        &self.planner
    }

    pub fn coder(&self) -> &FakeAcpAgent {
        &self.coder
    }

    pub fn agent(&self, name: &str) -> &FakeAcpAgent {
        match name {
            "Planner" => &self.planner,
            "Coder" => &self.coder,
            other => panic!("unknown fake ACP agent {other:?}"),
        }
    }
}

impl FakeAcpAgent {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Asserts the agent's most recent turn saw a conversation containing each
    /// of `expected` (user or assistant text), in addition to anything else.
    pub fn assert_saw(&self, expected: &[&str]) {
        let seen = self.latest_conversation();
        for text in expected {
            assert!(seen.iter().any(|m| m == text), "{} should have seen {text:?}; saw {seen:?}", self.name);
        }
    }

    /// Asserts the agent's most recent turn saw *exactly* `expected` and nothing
    /// else — used to prove a freshly-activated agent started with no prior
    /// transcript.
    pub fn assert_saw_exactly(&self, expected: &[&str]) {
        let seen = self.latest_conversation();
        let expected: Vec<String> = expected.iter().map(|t| (*t).to_string()).collect();
        assert_eq!(seen, expected, "{} conversation mismatch", self.name);
    }

    /// Asserts the agent never ran a turn (its LLM was never invoked).
    pub fn assert_never_ran(&self) {
        let contexts = self.captured_contexts.lock().expect("captured contexts lock is healthy");
        assert!(contexts.is_empty(), "{} should not have run; captured {} context(s)", self.name, contexts.len());
    }

    fn latest_conversation(&self) -> Vec<String> {
        let contexts = self.captured_contexts.lock().expect("captured contexts lock is healthy");
        let latest = contexts.last().unwrap_or_else(|| panic!("{} should have run a turn", self.name));
        conversation_texts(latest)
    }
}

/// Spawns each agent's runtime through the real [`AgentRuntime`] wiring, but
/// backed by a [`FakeLlmProvider`] and an in-memory MCP server instead of a
/// network LLM and external MCP processes.
struct FakeRuntimeFactory {
    cwd: PathBuf,
    agents: HashMap<String, FakeAgentDef>,
}

struct FakeAgentDef {
    spec: AgentSpec,
    provider: Mutex<Option<Arc<dyn StreamingModelProvider>>>,
    mcp: Option<(String, String)>,
}

#[async_trait::async_trait]
impl RuntimeFactory for FakeRuntimeFactory {
    async fn spawn(
        &self,
        agent: AgentKey,
        spec: &AgentSpec,
        initial_messages: Vec<ChatMessage>,
        runtime_event_tx: mpsc::Sender<RuntimeEvent>,
    ) -> Result<AgentRuntime, SessionError> {
        let def = self.agents.get(&spec.name).ok_or_else(|| SessionError::AgentNotFound(spec.name.clone()))?;
        let provider = def
            .provider
            .lock()
            .expect("fake provider lock is healthy")
            .take()
            .expect("fake agent runtime spawned more than once");

        let mut mcp_builder = mcp(&self.cwd).with_tool_filter(spec.tools.clone());
        if let Some((server_name, prompt_name)) = &def.mcp {
            let factory_name = server_name.clone();
            let prompt_name = prompt_name.clone();
            let factory: ServerFactory = Box::new(move |_spec, _services| {
                let prompt_name = prompt_name.clone();
                async move { FakePromptMcp::new(&prompt_name).into_dyn() }.boxed()
            });
            mcp_builder = mcp_builder.register_in_memory_server(factory_name.clone(), factory).with_servers(vec![
                McpServer::new(
                    server_name.clone(),
                    McpTransport::InMemory {
                        spec: InMemoryServerSpec { factory: factory_name, args: Vec::new(), input: None },
                    },
                    ToolExposure::ModelVisible,
                ),
            ]);
        }
        let mut spawn =
            mcp_builder.spawn().await.map_err(|e| SessionError::Build(CliError::McpError(e.to_string())))?;
        spawn
            .block_until_ready()
            .await
            .ok_or_else(|| SessionError::McpOperation("fake MCP bootstrap aborted".to_string()))?;
        let mcp_handle = spawn.handle().clone();
        let mut builder = AgentBuilder::new(provider).max_auto_continues(0);
        for prompt in &spec.prompts {
            builder = builder.system_prompt(prompt.clone());
        }
        let (agent_tx, agent_rx, agent_handle) = builder
            .tools(mcp_handle, Vec::new())
            .messages(initial_messages)
            .spawn()
            .await
            .map_err(|e| SessionError::Build(CliError::AgentError(e.to_string())))?;
        let (mcp_runtime, event_rx) = spawn.connect_agent(agent_tx.clone()).await.split();

        Ok(AgentRuntime::new(agent, agent_tx, agent_rx, Some(agent_handle), event_rx, mcp_runtime, runtime_event_tx))
    }
}

struct StubRuntimeFactory {
    cwd: PathBuf,
    agent_parts: Mutex<Option<StubAgentParts>>,
}

struct StubAgentParts {
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<AgentEvent>,
    handle: AgentHandle,
}

#[async_trait::async_trait]
impl RuntimeFactory for StubRuntimeFactory {
    async fn spawn(
        &self,
        agent: AgentKey,
        _spec: &AgentSpec,
        _initial_messages: Vec<ChatMessage>,
        runtime_event_tx: mpsc::Sender<RuntimeEvent>,
    ) -> Result<AgentRuntime, SessionError> {
        let parts = self
            .agent_parts
            .lock()
            .expect("stub agent parts lock is healthy")
            .take()
            .expect("stub runtime spawned more than once");

        let mut spawn =
            mcp(&self.cwd).spawn().await.map_err(|e| SessionError::Build(CliError::McpError(e.to_string())))?;
        spawn
            .block_until_ready()
            .await
            .ok_or_else(|| SessionError::McpOperation("stub MCP bootstrap aborted".to_string()))?;
        let (mcp_runtime, event_rx) = spawn.connect_agent(parts.tx.clone()).await.split();

        Ok(AgentRuntime::new(agent, parts.tx, parts.rx, Some(parts.handle), event_rx, mcp_runtime, runtime_event_tx))
    }
}

fn fake_agent(name: &str, server_name: &str, prompt_name: &str, reply: &str) -> (FakeAgentDef, FakeAcpAgent) {
    let provider =
        FakeLlmProvider::new(vec![vec![LlmResponse::start("msg"), LlmResponse::text(reply), LlmResponse::done()]])
            .with_display_name(name);
    let captured_contexts = provider.captured_contexts();
    let def = FakeAgentDef {
        spec: fake_agent_spec(name),
        provider: Mutex::new(Some(Arc::new(provider))),
        mcp: Some((server_name.to_string(), prompt_name.to_string())),
    };
    let observer = FakeAcpAgent { name: name.to_string(), captured_contexts };
    (def, observer)
}

fn fake_oauth_store() -> Arc<dyn OAuthCredentialStorage> {
    Arc::new(aether_auth::FakeOAuthCredentialStore::new())
}

fn switching_modes() -> Modes {
    Modes::new(vec![
        ValidatedMode {
            name: "Planner".to_string(),
            model: "anthropic:claude-sonnet-4-5".to_string(),
            reasoning_effort: None,
        },
        ValidatedMode {
            name: "Coder".to_string(),
            model: "deepseek:deepseek-chat".to_string(),
            reasoning_effort: None,
        },
    ])
}

fn fake_agent_spec(name: &str) -> AgentSpec {
    let model: llm::catalog::LlmModel = "anthropic:claude-sonnet-4-5".parse().expect("test model parses");
    let mut spec = AgentSpec::bare(&model, None, vec![Prompt::text(&format!("{name} system prompt"))]);
    spec.name = name.to_string();
    spec.description = format!("{name} test agent");
    spec.exposure = AgentSpecExposure::user_only();
    spec
}

fn assert_available_commands(update: SessionUpdate, expected: &[&str], unexpected: &[&str]) {
    let SessionUpdate::AvailableCommandsUpdate(commands) = update else {
        panic!("expected available commands update");
    };
    let names = commands.available_commands.iter().map(|command| command.name.as_str()).collect::<Vec<_>>();
    for name in expected {
        assert!(names.contains(name), "expected command /{name} in {names:?}");
    }
    for name in unexpected {
        assert!(!names.contains(name), "did not expect command /{name} in {names:?}");
    }
}

fn assert_server_status(notification: McpNotification, expected: &[&str]) {
    let McpNotification::ServerStatus { servers } = notification;
    let names = servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>();
    for server_name in expected {
        assert!(names.contains(server_name), "expected server {server_name} in {names:?}");
    }
}

fn assert_server_status_exact(notification: McpNotification, expected: &[&str]) {
    let McpNotification::ServerStatus { servers } = notification;
    let names = servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names, expected);
}

fn conversation_texts(context: &Context) -> Vec<String> {
    context
        .messages()
        .iter()
        .filter_map(|message| match message {
            ChatMessage::User { content, .. } => llm::ContentBlock::first_text(content).map(str::to_string),
            ChatMessage::Assistant { content, .. } if !content.is_empty() => Some(content.clone()),
            _ => None,
        })
        .collect()
}
