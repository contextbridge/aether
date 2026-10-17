use super::server::{AcpServer, ServerRunError};
use super::session::actor::SessionActorInit;
use super::session::agent_key::AgentKey;
use super::session::agents::SessionAgents;
use super::session::config::SessionConfigState;
use super::session::error::SessionError;
use super::session::factory::SessionFactoryConfig;
use super::session::hooks::SessionHooks;
use super::session::model::{Modes, ValidatedMode};
use super::session::runtime::{AgentRuntime, RuntimeFactory};
use super::state::{AcpState, AcpStateConfig};
use crate::resolve::InitialSessionSelection;
use crate::runtime::{Runtime, RuntimeBuilder};
use crate::settings_args::SettingsSourceArgs;
use crate::workspace::WorkspaceManager;
use crate::workspace::testing::StdCopyCloner;
use acp_utils::notifications::McpNotification;
use acp_utils::testing::{TestPeer, initialize_request};
use aether_auth::OAuthCredentialStorage;
use aether_core::agent_spec::{AgentSpec, AgentSpecExposure};
use aether_core::core::{AgentBuilder, Prompt};
use aether_core::events::{AgentEvent, AgentObserver, Command, MessageEvent, TurnOutcome, UserCommand};
use aether_core::mcp::{McpBuilder, McpCatalog, mcp};
use aether_core::testing::ready_catalog;
use aether_project::AgentCatalog;
use aether_sessions::SessionStore;
use aether_sessions::{SessionControlEvent, SessionEvent, SessionMeta, UserEvent, last_agent_from_events};
use agent_client_protocol::schema::v2::{
    AbsolutePath, AvailableCommand, ContentBlock, InitializeResponse, PromptRequest, PromptResponse, ReplayFrom,
    ReplayFromStart, ResumeSessionRequest, SessionId, SessionUpdate, StateUpdate, StopReason,
};
use agent_client_protocol::{Agent, Channel, Client, ConnectionTo};
use llm::testing::FakeLlmProvider;
use llm::{ChatMessage, Context, LlmModel, LlmResponse, SessionUsageEvent, StreamingModelProvider};
use llm::{MessageId, ProviderConnectionOverrides};
use mcp_utils::client::{Elicitation, ElicitationSink};
use mcp_utils::gateway::ServerSpec;
use mcp_utils::model::{CallToolResult, CreateTaskResult, DetailedTask, ElicitResult, Task, TaskPayload, TaskStatus};
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse, Gate, completed_task_payload, elicitation_sink};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet, LocalSet};
use tokio_util::sync::CancellationToken;
use utils::mcp_status::McpServerStatusEntry;

const PLANNER_REPLY: &str = "planner reply";
const CODER_REPLY: &str = "coder reply";

pub struct AcpTestHarness {
    pub client_cx: ConnectionTo<Agent>,
    pub peer: TestPeer,
    pub initialize_response: InitializeResponse,
    connection: Option<HarnessConnectionTasks>,
    resume_agent: FakeAcpAgent,
    runtime_control: Arc<Mutex<FakeRuntimeControl>>,
    pub oauth_store: Arc<aether_auth::MemoryCredentialStore>,
    agent_cx: ConnectionTo<Client>,
    state: Arc<AcpState>,
    session_store: Arc<SessionStore>,
    _tmp: tempfile::TempDir,
}

/// A real loopback listener backed by the harness's existing state and fake runtimes.
pub struct AcpWebSocketTestServer {
    pub address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), ServerRunError>>,
}

impl AcpWebSocketTestServer {
    pub fn url(&self) -> String {
        format!("ws://{}", self.address)
    }

    /// Join networking cleanup without shutting down the harness-owned host.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown.take().expect("server is running").send(());
        (&mut self.task).await.expect("listener task joins").expect("listener shuts down");
    }

    /// Drop the running server without executing the normal async shutdown path.
    pub async fn abort(mut self) {
        self.task.abort();
        assert!((&mut self.task).await.expect_err("listener task is aborted").is_cancelled());
    }
}

impl Drop for AcpWebSocketTestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
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

pub struct FakeBackgroundTask {
    server: FakeMcpServer,
}

/// Builds a session served by one fake agent; see [`AcpTestHarness::session`].
pub struct TestSessionBuilder<'a> {
    harness: &'a AcpTestHarness,
    id: SessionId,
    provider: FakeLlmProvider,
    model: String,
    servers: Vec<ServerSpec>,
    max_auto_continues: Option<u32>,
    observer: Option<Box<dyn AgentObserver>>,
}

impl TestSessionBuilder<'_> {
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    pub fn observer(mut self, observer: impl AgentObserver + 'static) -> Self {
        self.observer = Some(Box::new(observer));
        self
    }

    /// Serve `task` and a `/plan` prompt, and let the agent end its turn
    /// without auto-continuing so the task outcome arrives as its own turn.
    pub fn background_task(mut self, task: &FakeBackgroundTask) -> Self {
        let prompts = self.harness.runtime_control.lock().unwrap().prompt_server("plan");
        self.servers.push(ServerSpec::in_process("tasks", task.server.clone()));
        self.servers.push(ServerSpec::in_process("prompts", prompts));
        self.max_auto_continues = Some(0);
        self
    }

    pub async fn insert(self) -> SessionId {
        let Self { harness, id, provider, model, servers, max_auto_continues, observer } = self;
        let (mcp, elicitations) = spawn_ready(mcp("/tmp").with_servers(servers)).await;
        let mut agent = AgentBuilder::new(Arc::new(provider)).mcp(mcp.clone());
        if let Some(max) = max_auto_continues {
            agent = agent.max_auto_continues(max);
        }
        if let Some(observer) = observer {
            agent = agent.observer(observer);
        }
        let (agent_tx, agent_rx, agent_handle) = agent.spawn().await.expect("fake agent spawns");

        let control = &harness.runtime_control;
        control.lock().unwrap().agents.push(agent_tx.clone());
        let agent_tx = count_prompts(agent_tx, control.lock().unwrap().agent_prompts.clone());
        let runtime = Runtime { agent_tx, agent_rx, agent_handle, mcp };
        let factory = Arc::new(StubRuntimeFactory { runtime: Mutex::new(Some((runtime, elicitations))) });

        let default_model: llm::catalog::LlmModel = "anthropic:claude-sonnet-4-5".parse().expect("test model parses");
        let mut specs = SessionAgents::new(AgentCatalog::empty(PathBuf::from("/tmp")));
        specs.set_default(AgentSpec::bare(&default_model, None, Vec::new()));
        let config = SessionConfigState::with_selection(model, None, None);
        harness.state.register_session(harness.session_init(id.clone(), specs, factory, config)).await;
        id
    }
}

pub struct PendingRuntime {
    started: oneshot::Receiver<()>,
    release: oneshot::Sender<bool>,
}

impl PendingRuntime {
    pub async fn wait_until_started(&mut self) {
        (&mut self.started).await.expect("runtime startup reached");
    }

    pub fn finish(self, succeed: bool) {
        self.release.send(succeed).expect("runtime startup is waiting");
    }
}

impl AcpTestHarness {
    pub fn pause_next_runtime(&self) -> PendingRuntime {
        let (started, observed) = oneshot::channel();
        let (release, proceed) = oneshot::channel();
        self.runtime_control.lock().unwrap().pending = Some((started, proceed));
        PendingRuntime { started: observed, release }
    }

    pub fn pause_prompt_expansion(&self) -> Gate {
        let gate = Gate::new();
        self.runtime_control.lock().unwrap().prompt_gate = Some(gate.clone());
        gate
    }

    pub fn add_mcp_server(&self, server: ServerSpec) {
        self.runtime_control.lock().unwrap().extra_servers.push(server);
    }

    pub fn add_stalled_mcp_server(&self) {
        self.runtime_control.lock().unwrap().stalled_server = true;
    }

    pub fn watch_agent_prompts(&self) -> watch::Receiver<usize> {
        self.runtime_control.lock().unwrap().agent_prompts.subscribe()
    }

    pub fn elicit_during_prompt_expansion(&self) -> mpsc::UnboundedReceiver<ElicitResult> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.runtime_control.lock().unwrap().elicitation_results = Some(sender);
        receiver
    }

    pub fn live_runtime_count(&self) -> usize {
        self.runtime_control.lock().unwrap().agents.iter().filter(|sender| !sender.is_closed()).count()
    }

    pub async fn disconnect(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.shutdown().await;
        }
    }

    pub async fn start_unattached_session(&mut self, prompt: impl Into<String>) -> SessionId {
        self.disconnect().await;
        self.state.start_session(prompt.into()).await.expect("unattached session starts")
    }

    /// Connect a fresh initialized client to the same host and session store.
    pub async fn reconnect(&mut self) {
        assert!(self.connection.is_none(), "disconnect the current client before reconnecting");
        assert!(!self.state.stop_token().is_cancelled(), "host is running");
        let connection = connect_client(self.state.clone()).await;
        self.client_cx = connection.client_cx;
        self.agent_cx = connection.agent_cx;
        self.peer = connection.peer;
        self.initialize_response = connection.initialize_response;
        self.connection = Some(connection.tasks);
    }

    pub async fn resume(&self, session_id: &SessionId) {
        self.client_cx
            .send_request(ResumeSessionRequest::new(session_id.clone(), AbsolutePath::new("/tmp")))
            .block_task()
            .await
            .expect("session resumes");
    }

    pub async fn resume_with_replay(&self, session_id: &SessionId) {
        self.client_cx
            .send_request(
                ResumeSessionRequest::new(session_id.clone(), AbsolutePath::new("/tmp"))
                    .replay_from(ReplayFrom::Start(ReplayFromStart::new())),
            )
            .block_task()
            .await
            .expect("session resumes with replay");
    }

    pub fn prompt<C: Into<ContentBlock>>(
        &self,
        session_id: &SessionId,
        content: C,
    ) -> impl Future<Output = Result<PromptResponse, agent_client_protocol::Error>> + use<C> {
        self.client_cx.send_request(PromptRequest::new(session_id.clone(), vec![content.into()])).block_task()
    }

    /// Move this persistent host from its in-memory connection to a real port-zero listener.
    pub async fn serve_websocket(&mut self) -> AcpWebSocketTestServer {
        self.disconnect().await;
        self.listen_websocket().await
    }

    /// Listen without disconnecting any existing client.
    pub async fn listen_websocket(&self) -> AcpWebSocketTestServer {
        assert!(!self.state.stop_token().is_cancelled(), "host is running");
        let server =
            AcpServer::bind("127.0.0.1:0".parse().unwrap(), self.state.clone()).await.expect("bind test server");
        let address = server.local_addr().expect("listener address");
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(server.run_until(async move {
            let _ = stopped.await;
            Ok(())
        }));
        AcpWebSocketTestServer { address, shutdown: Some(shutdown), task }
    }

    pub async fn until_detached(&self, action: impl Future<Output = ()>) {
        let mut detached = self.state.client_slot().subscribe();
        let seen = *detached.borrow();
        action.await;
        detached.wait_for(|generation| *generation > seen).await.expect("host client slot is alive");
    }

    /// Stop the connection and all host-owned session runtimes.
    pub async fn shutdown(&mut self) {
        self.disconnect().await;
        self.state.shutdown_all().await;
    }

    pub fn stored_events(&self, session_id: &SessionId) -> Vec<SessionEvent> {
        self.session_store.load(session_id.0.as_ref()).expect("stored session loads").1
    }

    /// Runs `body` with a fresh harness on the current-thread `LocalSet` the
    /// host needs for its `spawn_local` tasks; tests must be annotated with
    /// `#[tokio::test(flavor = "current_thread")]`.
    pub async fn run<F, Fut, T>(body: F) -> T
    where
        F: FnOnce(Self) -> Fut,
        Fut: Future<Output = T>,
    {
        Self::run_with_hooks(SessionHooks::default(), body).await
    }

    pub async fn run_with_hooks<F, Fut, T>(hooks: SessionHooks, body: F) -> T
    where
        F: FnOnce(Self) -> Fut,
        Fut: Future<Output = T>,
    {
        LocalSet::new().run_until(Box::pin(async move { body(Self::start(hooks).await).await })).await
    }

    async fn start(hooks: SessionHooks) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir for session store");
        let session_store = Arc::new(SessionStore::from_path(tmp.path().to_path_buf()));
        let workspace_manager = Arc::new(WorkspaceManager::from_registry_path_with_cloner(
            tmp.path().join("workspaces.json"),
            Arc::new(StdCopyCloner),
        ));
        let (resume_def, resume_agent) = fake_agent("Resume", "resume-mcp", "resume", "resumed reply");
        let mut resume_agents = HashMap::new();
        resume_agents.insert(resume_def.spec.name.clone(), resume_def);
        let runtime_control = Arc::new(Mutex::new(FakeRuntimeControl::default()));
        let runtime_factory = Arc::new(FakeRuntimeFactory {
            cwd: PathBuf::from("/tmp"),
            agents: resume_agents,
            control: runtime_control.clone(),
        });
        let oauth_store = Arc::new(aether_auth::MemoryCredentialStore::new());
        let state = Arc::new(AcpState::with_login(
            AcpStateConfig {
                session_store: session_store.clone(),
                workspace_manager,
                oauth_credential_store: oauth_store.clone(),
                telemetry: None,
                cwd: PathBuf::from("/tmp"),
                sessions: SessionFactoryConfig {
                    initial_selection: InitialSessionSelection::default(),
                    settings_source: SettingsSourceArgs {
                        settings_json: Some(r#"{"agents":[]}"#.to_string()),
                        settings_file: None,
                    },
                    provider_connections: ProviderConnectionOverrides::default(),
                    runtime_factory: Some(runtime_factory),
                    hooks,
                },
            },
            Arc::new(FakeProviderLogin),
        ));

        let connection = connect_client(state.clone()).await;
        Self {
            client_cx: connection.client_cx,
            peer: connection.peer,
            initialize_response: connection.initialize_response,
            connection: Some(connection.tasks),
            resume_agent,
            runtime_control,
            oauth_store,
            agent_cx: connection.agent_cx,
            state,
            session_store,
            _tmp: tmp,
        }
    }

    pub fn resume_agent(&self) -> &FakeAcpAgent {
        &self.resume_agent
    }

    pub async fn insert_agent_switching_session(&self) -> FakeAgentSwitchingSession {
        self.insert_switching_session("agent-switching-session", Vec::new(), Some("Planner".into()), false).await
    }

    pub async fn insert_agent_switching_session_with_serverless_coder(&self) -> FakeAgentSwitchingSession {
        self.insert_switching_session("agent-switching-serverless-session", Vec::new(), Some("Planner".into()), true)
            .await
    }

    pub async fn insert_loaded_agent_switching_session(&self, session_id: &str) -> FakeAgentSwitchingSession {
        let events = self.session_store.load(session_id).map(|(_, events)| events).unwrap_or_default();
        let selected_mode = last_agent_from_events(Some("Planner".to_string()), &events);
        self.insert_switching_session(session_id, events, selected_mode, false).await
    }

    pub async fn expect_idle(&mut self, session_id: &SessionId, expected: StopReason) {
        loop {
            let notification = self.peer.next_session_notification().await;
            if notification.session_id == *session_id
                && let SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) = notification.update
            {
                assert_eq!(idle.stop_reason, Some(expected));
                return;
            }
        }
    }

    pub async fn updates_until(
        &mut self,
        session_id: &SessionId,
        last: impl Fn(&SessionUpdate) -> bool,
    ) -> Vec<SessionUpdate> {
        let mut updates = Vec::new();
        loop {
            let notification = self.peer.next_session_notification().await;
            if notification.session_id != *session_id {
                continue;
            }
            let done = last(&notification.update);
            updates.push(notification.update);
            if done {
                return updates;
            }
        }
    }

    pub async fn expect_mcp_server_status(&mut self, expected: &[&str]) {
        let statuses = self.next_mcp_server_statuses().await;
        let names = statuses.iter().map(|server| server.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, expected);
    }

    pub async fn next_mcp_server_statuses(&mut self) -> Vec<McpServerStatusEntry> {
        let McpNotification::ServerStatus { servers } = self.peer.next_mcp_notification().await;
        servers
    }

    pub async fn expect_available_commands(&mut self, expected: &[&str], unexpected: &[&str]) {
        let commands = self.next_available_commands().await;
        let names = commands.iter().map(|command| command.name.as_str()).collect::<Vec<_>>();
        for name in expected {
            assert!(names.contains(name), "expected command /{name} in {names:?}");
        }
        for name in unexpected {
            assert!(!names.contains(name), "did not expect command /{name} in {names:?}");
        }
    }

    pub async fn next_available_commands(&mut self) -> Vec<AvailableCommand> {
        loop {
            if let SessionUpdate::AvailableCommandsUpdate(update) = self.peer.next_session_notification().await.update {
                return update.available_commands;
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

    /// Start building a session whose agent streams from `provider`.
    pub fn session(&self, id: impl Into<SessionId>, provider: FakeLlmProvider) -> TestSessionBuilder<'_> {
        TestSessionBuilder {
            harness: self,
            id: id.into(),
            provider,
            model: "fake:fake".to_string(),
            servers: Vec::new(),
            max_auto_continues: None,
            observer: None,
        }
    }

    pub fn append_stored_session(&self, session_id: &str, created_at: &str) {
        self.append_stored_session_in(session_id, created_at, std::path::Path::new("/tmp"));
    }

    pub fn append_stored_session_in(&self, session_id: &str, created_at: &str, cwd: &std::path::Path) {
        let meta = SessionMeta {
            session_id: session_id.to_string(),
            cwd: cwd.to_path_buf(),
            model: "anthropic:claude-sonnet-4-5".to_string(),
            selected_mode: None,
            created_at: created_at.to_string(),
        };

        self.session_store.append_meta(session_id, &meta).expect("stored session meta appends");
    }

    pub fn append_stored_prompt(&self, session_id: &str, prompt: &str) {
        self.append_stored_user_blocks(session_id, vec![llm::ContentBlock::text(prompt)]);
    }

    pub fn append_stored_user_blocks(&self, session_id: &str, blocks: Vec<llm::ContentBlock>) {
        self.append_stored_event(
            session_id,
            &SessionEvent::User(UserEvent::Message {
                message_id: llm::MessageId::new(),
                content: blocks,
                display_content: None,
            }),
        );
    }

    pub fn append_stored_agent_turn(&self, session_id: &str, text: &str) {
        self.append_stored_agent_text(session_id, text);
        self.append_stored_event(session_id, &SessionEvent::Agent(AgentEvent::turn_ended(TurnOutcome::Completed)));
    }

    pub fn append_stored_agent_text(&self, session_id: &str, text: &str) {
        self.append_stored_event(
            session_id,
            &SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
                message_id: MessageId::new(),
                chunk: text.to_string(),
                is_complete: true,
            })),
        );
    }

    pub fn append_stored_tool_call(&self, session_id: &str, id: &str, name: &str, arguments: &str) {
        self.append_stored_event(session_id, &SessionEvent::Agent(AgentEvent::tool_call(id, name, arguments)));
    }

    pub fn append_stored_tool_result(&self, session_id: &str, id: &str, name: &str, result: &str) {
        self.append_stored_event(session_id, &SessionEvent::Agent(AgentEvent::tool_result(id, name, "{}", result)));
    }

    async fn insert_switching_session(
        &self,
        session_id: &str,
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

        let factory =
            Arc::new(FakeRuntimeFactory { cwd: PathBuf::from("/tmp"), agents, control: self.runtime_control.clone() });
        let initial_agent = selected_mode.clone().unwrap_or_else(|| "Planner".to_string());
        let session_id = SessionId::new(session_id);
        let config = SessionConfigState::with_selection("anthropic:claude-sonnet-4-5".to_string(), selected_mode, None);
        let init = SessionActorInit {
            active_agent: AgentKey::Named(initial_agent),
            transcript: events,
            modes: switching_modes(),
            ..self.session_init(session_id.clone(), specs, factory, config)
        };
        self.state.register_session(init).await;
        FakeAgentSwitchingSession { session_id, planner, coder }
    }

    pub fn append_stored_event(&self, session_id: &str, event: &SessionEvent) {
        self.session_store.append_event(session_id, event).expect("stored session event appends");
    }

    fn session_init(
        &self,
        session_id: SessionId,
        specs: SessionAgents,
        runtime_factory: Arc<dyn RuntimeFactory>,
        config: SessionConfigState,
    ) -> SessionActorInit {
        SessionActorInit {
            session_id,
            cwd: PathBuf::from("/tmp"),
            mcp_servers: Vec::new(),
            connection: Some(self.agent_cx.clone()),
            repository: self.session_store.clone(),
            oauth_credential_store: self.oauth_store.clone(),
            active_agent: AgentKey::Default,
            specs,
            runtime_factory,
            transcript: Vec::new(),
            replay: false,
            modes: Modes::default(),
            available: LlmModel::all().into(),
            config,
            hooks: SessionHooks::default(),
        }
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
}

impl FakeAcpAgent {
    /// Asserts the agent's most recent turn saw a conversation containing each
    /// of `expected` (user or assistant text), in addition to anything else.
    pub fn assert_saw(&self, expected: &[&str]) {
        let seen = conversation_texts(&self.latest_context());
        for text in expected {
            assert!(seen.iter().any(|m| m == text), "{} should have seen {text:?}; saw {seen:?}", self.name);
        }
    }

    /// Asserts the agent's most recent turn saw *exactly* `expected` and nothing
    /// else — used to prove a freshly-activated agent started with no prior
    /// transcript.
    pub fn assert_saw_exactly(&self, expected: &[&str]) {
        let seen = conversation_texts(&self.latest_context());
        let expected: Vec<String> = expected.iter().map(|t| (*t).to_string()).collect();
        assert_eq!(seen, expected, "{} conversation mismatch", self.name);
    }

    pub fn assert_saw_user_content(&self, expected: &[llm::ContentBlock]) {
        let latest = self.latest_context();
        assert!(
            latest
                .messages()
                .iter()
                .any(|message| matches!(message, ChatMessage::User { content, .. } if content == expected)),
            "{} should have seen user content {expected:?}",
            self.name
        );
    }

    /// Asserts the agent never ran a turn (its LLM was never invoked).
    pub fn assert_never_ran(&self) {
        let contexts = self.captured_contexts.lock().expect("captured contexts lock is healthy");
        assert!(contexts.is_empty(), "{} should not have run; captured {} context(s)", self.name, contexts.len());
    }

    fn latest_context(&self) -> Context {
        let contexts = self.captured_contexts.lock().expect("captured contexts lock is healthy");
        contexts.last().cloned().unwrap_or_else(|| panic!("{} should have run a turn", self.name))
    }
}

impl FakeBackgroundTask {
    pub const TOOL: &str = "tasks__start";
    const ID: &str = "background-task";

    pub fn new() -> Self {
        let server = FakeMcpServer::new()
            .with_tool(Self::start_tool())
            .with_task(Self::ID, [DetailedTask::new(background_task(TaskStatus::Working), TaskPayload::Working)]);
        Self { server }
    }

    pub fn complete(&self, output: &str) {
        let result = CallToolResult::success(vec![mcp_utils::model::ContentBlock::text(output)]);
        let completed = DetailedTask::new(background_task(TaskStatus::Completed), completed_task_payload(result));
        self.server.state().script_task(Self::ID, [completed]);
    }

    pub async fn announce_unchanged_tools(&self) {
        let state = self.server.state();
        let listed = state.tool_list_requests();
        state.notify_tools_changed().await;
        state.wait_until(|state| state.tool_list_requests() > listed).await;
    }

    pub async fn add_tool(&self, name: &str) {
        self.server.state().add_tool_and_notify(FakeTool::new(name)).await;
    }

    fn start_tool() -> FakeTool {
        let working = background_task(TaskStatus::Working);
        FakeTool::new("start").responds(FakeToolResponse::task(CreateTaskResult::new(working)))
    }
}

impl Default for FakeBackgroundTask {
    fn default() -> Self {
        Self::new()
    }
}

struct HarnessConnection {
    client_cx: ConnectionTo<Agent>,
    agent_cx: ConnectionTo<Client>,
    peer: TestPeer,
    initialize_response: InitializeResponse,
    tasks: HarnessConnectionTasks,
}

struct HarnessConnectionTasks {
    stop: CancellationToken,
    tasks: JoinSet<Result<(), agent_client_protocol::Error>>,
}

impl HarnessConnectionTasks {
    async fn shutdown(mut self) {
        self.stop.cancel();
        while let Some(result) = self.tasks.join_next().await {
            result.expect("connection task joins").expect("connection cleanup completes");
        }
    }
}

async fn connect_client(state: Arc<AcpState>) -> HarnessConnection {
    let (peer, client_builder) = TestPeer::new();
    let (agent_transport, client_transport) = Channel::duplex();
    let (send_agent_connection, agent_ready) = oneshot::channel();
    let (send_client_connection, client_ready) = oneshot::channel();
    let stop = state.stop_token().child_token();
    let mut tasks = JoinSet::new();
    let agent_stop = stop.clone();
    tasks.spawn_local(async move { state.serve_with(agent_transport, agent_stop, send_agent_connection).await });
    tasks.spawn_local(async move {
        client_builder
            .with_runner(acp_utils::testing::CaptureConnection(send_client_connection))
            .connect_to(client_transport)
            .await
    });
    let agent_cx = agent_ready.await.expect("agent connection");
    let client_cx = client_ready.await.expect("client connection");
    let initialize_response =
        client_cx.send_request(initialize_request()).block_task().await.expect("initialize harness");
    HarnessConnection { client_cx, agent_cx, peer, initialize_response, tasks: HarnessConnectionTasks { stop, tasks } }
}

/// Spawns each agent's runtime through the real [`AgentRuntime`] wiring, but
/// backed by a [`FakeLlmProvider`] and an in-memory MCP server instead of a
/// network LLM and external MCP processes.
struct FakeRuntimeFactory {
    cwd: PathBuf,
    agents: HashMap<String, FakeAgentDef>,
    control: Arc<Mutex<FakeRuntimeControl>>,
}

#[derive(Default)]
struct FakeRuntimeControl {
    agents: Vec<mpsc::Sender<Command>>,
    pending: Option<(oneshot::Sender<()>, oneshot::Receiver<bool>)>,
    prompt_gate: Option<Gate>,
    elicitation_results: Option<mpsc::UnboundedSender<ElicitResult>>,
    agent_prompts: watch::Sender<usize>,
    extra_servers: Vec<ServerSpec>,
    stalled_server: bool,
}

impl FakeRuntimeControl {
    fn servers(&self, def: &FakeAgentDef) -> Vec<ServerSpec> {
        let mut servers = def
            .mcp
            .iter()
            .map(|(server_name, prompt_name)| ServerSpec::in_process(server_name, self.prompt_server(prompt_name)))
            .collect::<Vec<_>>();
        servers.extend(self.extra_servers.iter().cloned());
        if self.stalled_server {
            servers.push(ServerSpec::in_process("stalled-mcp", FakeMcpServer::new().with_tool_list_gate(Gate::new())));
        }
        servers
    }

    fn prompt_server(&self, prompt_name: &str) -> FakeMcpServer {
        let mut server = FakeMcpServer::new().with_prompt(prompt_name, format!("expanded {prompt_name}"));
        if let Some(gate) = self.prompt_gate.clone() {
            server = server.with_prompt_gate(gate);
        }
        if let Some(results) = self.elicitation_results.clone() {
            server = server.with_prompt_elicitation(results);
        }
        server
    }
}

struct FakeAgentDef {
    spec: AgentSpec,
    provider: Arc<dyn StreamingModelProvider>,
    mcp: Option<(String, String)>,
}

#[async_trait::async_trait]
impl RuntimeFactory for FakeRuntimeFactory {
    async fn spawn(
        &self,
        spec: &AgentSpec,
        initial_messages: Vec<ChatMessage>,
        usage_seed: Option<SessionUsageEvent>,
        elicitations: mpsc::Sender<Elicitation>,
    ) -> Result<AgentRuntime, SessionError> {
        let pending = self.control.lock().unwrap().pending.take();
        if let Some((started, proceed)) = pending {
            let _ = started.send(());
            if !proceed.await.unwrap_or(false) {
                return Err(SessionError::AgentNotFound("injected startup failure".to_string()));
            }
        }
        let def = self
            .agents
            .get(&spec.name)
            .or_else(|| self.agents.values().next())
            .ok_or_else(|| SessionError::AgentNotFound(spec.name.clone()))?;
        let (servers, stalled_server) = {
            let control = self.control.lock().unwrap();
            (control.servers(def), control.stalled_server)
        };
        let mut builder = RuntimeBuilder::from_spec(self.cwd.clone(), spec.clone())
            .provider(def.provider.clone())
            .extra_servers(servers)
            .elicitation(ElicitationSink::form_and_url(elicitations))
            .resume_usage(usage_seed);

        if !stalled_server {
            builder = builder.wait_for_mcp();
        }

        let runtime = builder.build(initial_messages).await?;
        self.control.lock().unwrap().agents.push(runtime.agent_tx.clone());

        Ok(AgentRuntime::new(runtime))
    }
}

struct StubRuntimeFactory {
    runtime: Mutex<Option<(Runtime, mpsc::Receiver<Elicitation>)>>,
}

#[async_trait::async_trait]
impl RuntimeFactory for StubRuntimeFactory {
    async fn spawn(
        &self,
        _spec: &AgentSpec,
        _initial_messages: Vec<ChatMessage>,
        _usage_seed: Option<SessionUsageEvent>,
        elicitations: mpsc::Sender<Elicitation>,
    ) -> Result<AgentRuntime, SessionError> {
        let (runtime, mut stub_elicitations) =
            self.runtime.lock().expect("stub runtime lock is healthy").take().expect("stub runtime spawned once");
        tokio::spawn(async move {
            while let Some(elicitation) = stub_elicitations.recv().await {
                if elicitations.send(elicitation).await.is_err() {
                    return;
                }
            }
        });
        Ok(AgentRuntime::new(runtime))
    }
}

async fn spawn_ready(builder: McpBuilder) -> (McpCatalog, mpsc::Receiver<Elicitation>) {
    let (sink, elicitations) = elicitation_sink();
    (ready_catalog(builder.with_elicitation(sink)).await, elicitations)
}

fn count_prompts(agent_tx: mpsc::Sender<Command>, prompts: watch::Sender<usize>) -> mpsc::Sender<Command> {
    let (tx, mut rx) = mpsc::channel(32);
    tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            let is_prompt = matches!(command, Command::UserCommand(UserCommand::Text { .. }));
            if agent_tx.send(command).await.is_err() {
                return;
            }
            if is_prompt {
                prompts.send_modify(|count| *count += 1);
            }
        }
    });
    tx
}

fn background_task(status: TaskStatus) -> Task {
    const CREATED_AT: &str = "2026-01-01T00:00:00Z";
    Task::new(FakeBackgroundTask::ID, status, CREATED_AT.to_string(), CREATED_AT.to_string()).with_poll_interval_ms(10)
}

fn fake_agent(name: &str, server_name: &str, prompt_name: &str, reply: &str) -> (FakeAgentDef, FakeAcpAgent) {
    const TURNS_BEFORE_AND_AFTER_REATTACH: usize = 2;
    let provider = FakeLlmProvider::new(vec![
        vec![LlmResponse::Start, LlmResponse::text(reply), LlmResponse::done()];
        TURNS_BEFORE_AND_AFTER_REATTACH
    ])
    .with_display_name(name);
    let captured_contexts = provider.captured_contexts();
    let def = FakeAgentDef {
        spec: fake_agent_spec(name),
        provider: Arc::new(provider),
        mcp: Some((server_name.to_string(), prompt_name.to_string())),
    };
    let observer = FakeAcpAgent { name: name.to_string(), captured_contexts };
    (def, observer)
}

struct FakeProviderLogin;

#[async_trait::async_trait]
impl super::state::ProviderLogin for FakeProviderLogin {
    async fn login(&self, store: &dyn OAuthCredentialStorage) -> Result<(), llm::LlmError> {
        store
            .save("codex", serde_json::json!({"access_token": "fake-access", "refresh_token": "fake-refresh"}))
            .await?;
        Ok(())
    }
}

fn switching_modes() -> Modes {
    Modes::new(vec![
        ValidatedMode {
            name: "Planner".to_string(),
            model: "anthropic:claude-sonnet-4-5".to_string(),
            reasoning_effort: None,
        },
        ValidatedMode { name: "Coder".to_string(), model: text_only_model(), reasoning_effort: None },
    ])
}

fn text_only_model() -> String {
    LlmModel::all()
        .iter()
        .find(|model| !model.supports_image())
        .expect("catalog contains a text-only model")
        .to_string()
}

fn fake_agent_spec(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        description: format!("{name} test agent"),
        model: "anthropic:claude-sonnet-4-5".to_string(),
        prompts: vec![Prompt::text(&format!("{name} system prompt"))],
        exposure: AgentSpecExposure::user_only(),
        ..AgentSpec::default()
    }
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
