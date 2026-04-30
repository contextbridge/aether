use super::handlers::acp_agent_builder;
use super::relay::spawn_relay;
use super::session::Session;
use super::session_manager::{InitialSessionSelection, SessionManager, SessionManagerConfig};
use super::session_registry::SessionRegistry;
use super::session_store::SessionStore;
use crate::settings_args::SettingsSourceArgs;
use acp_utils::testing::{TestPeer, duplex_pair};
use aether_core::core::AgentHandle;
use aether_core::events::{AgentMessage, UserMessage};
use agent_client_protocol::schema::SessionId;
use agent_client_protocol::{Agent, Client, ConnectionTo};
use llm::oauth::OAuthCredentialStore;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::spawn_local;

/// In-memory ACP harness running the real `acp_agent_builder` against a
/// pre-wired test client. Created via [`AcpTestHarness::start`] inside a
/// `LocalSet`. The harness owns its own [`SessionRegistry`] and a
/// temp-dir-backed [`SessionStore`] so tests can register fake-driven
/// sessions without going through `new_session`.
pub struct AcpTestHarness {
    pub client_cx: ConnectionTo<Agent>,
    pub peer: TestPeer,
    agent_cx: ConnectionTo<Client>,
    registry: Arc<SessionRegistry>,
    session_store: Arc<SessionStore>,
    _tmp: TempDir,
}

impl AcpTestHarness {
    pub async fn start() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir for session store");
        let registry = Arc::new(SessionRegistry::new());
        let session_store = Arc::new(SessionStore::from_path(tmp.path().to_path_buf()));
        let manager = Arc::new(SessionManager::new(SessionManagerConfig {
            registry: registry.clone(),
            session_store: session_store.clone(),
            has_oauth_credential: OAuthCredentialStore::has_credential,
            initial_selection: InitialSessionSelection::default(),
            settings_source: SettingsSourceArgs::default(),
        }));

        let (peer, client_builder) = TestPeer::new();
        let (agent_transport, client_transport) = duplex_pair();
        let (agent_cx_tx, agent_cx_rx) = oneshot::channel::<ConnectionTo<Client>>();
        let (client_cx_tx, client_cx_rx) = oneshot::channel::<ConnectionTo<Agent>>();

        spawn_local(async move {
            let _ = acp_agent_builder(manager)
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
        Self { client_cx, peer, agent_cx, registry, session_store, _tmp: tmp }
    }

    /// Register a stub session built from a hand-spawned
    /// `(agent_tx, agent_rx, agent_handle)` triple — typically from
    /// `aether_core::core::agent(fake_llm).spawn().await`. MCP channels are
    /// stubbed: no servers, no events. The session is routable via
    /// `mgr.prompt(id)` / `mgr.cancel(id)`.
    pub async fn insert_stub_session(
        &self,
        agent_tx: mpsc::Sender<UserMessage>,
        agent_rx: mpsc::Receiver<AgentMessage>,
        agent_handle: AgentHandle,
        id: SessionId,
        model: &str,
    ) {
        let (mcp_tx, _mcp_rx) = mpsc::channel(1);
        let (_event_tx, event_rx) = mpsc::channel(1);
        let session = Session {
            agent_tx,
            agent_rx,
            agent_handle,
            _mcp_handle: tokio::spawn(async {}),
            mcp_tx,
            event_rx,
            initial_server_statuses: vec![],
        };
        let relay = spawn_relay(session, self.agent_cx.clone(), id.clone(), self.session_store.clone());
        self.registry.insert(id.0.to_string(), relay, model.to_string(), None, None, vec![]).await;
    }
}
