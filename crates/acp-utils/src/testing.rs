//! In-memory test harness for ACP connections.

mod fake_agent;
pub use fake_agent::{FakeAgent, FakeAgentRequests, HeldRequests, Question, QueuedTurns};

use crate::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use crate::notifications::{
    AetherMeta, AuthMethodsUpdatedParams, GitDiffEventPayload, McpNotification, PromptSearchResponse,
    PromptSearchResult, SessionPreviewResponse, SessionPreviewRole, SessionPreviewTurn, SubAgentMeta,
};
pub use agent_client_protocol::Channel;
use agent_client_protocol::schema::v2::{
    CompleteElicitationNotification, ContentBlock, ContentChunk, CreateElicitationRequest, CreateElicitationResponse,
    ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, IdleStateUpdate, Implementation,
    InitializeRequest, InitializeResponse, PlanEntry, PlanId, PlanUpdate, PlanUpdateContent, RunningStateUpdate,
    SessionConfigOption, SessionConfigSelectGroup, SessionConfigSelectOption, SessionConfigSelectOptions, SessionId,
    SessionUpdate, StateUpdate, StopReason, SubagentUpdate, ToolCallStatus, ToolCallUpdate, UpdateSessionNotification,
    UserMessage,
};
use agent_client_protocol::schema::{IntoOption, ProtocolVersion};
use agent_client_protocol::{
    self as acp, Agent, Client, ConnectionTo, HandleConnectionClose, HandleDispatchFrom, NullRun, Responder,
    RunWithConnectionTo, V2Builder,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::spawn_local;

pub struct TestPeer {
    session_notifications: mpsc::UnboundedReceiver<UpdateSessionNotification>,
    mcp_notifications: mpsc::UnboundedReceiver<McpNotification>,
    git_diff_notifications: mpsc::UnboundedReceiver<GitDiffEventPayload>,
    auth_methods_updates: mpsc::UnboundedReceiver<AuthMethodsUpdatedParams>,
    elicitation_requests: mpsc::UnboundedReceiver<CreateElicitationRequest>,
    elicitation_completions: mpsc::UnboundedReceiver<CompleteElicitationNotification>,
    elicitation_responses: Arc<Mutex<VecDeque<CreateElicitationResponse>>>,
    responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<CreateElicitationResponse>>>>>,
}

impl TestPeer {
    pub fn new() -> (Self, V2Builder<Client, impl HandleDispatchFrom<Agent>, NullRun>) {
        let (session_tx, session_notifications) = mpsc::unbounded_channel();
        let (mcp_tx, mcp_notifications) = mpsc::unbounded_channel();
        let (git_diff_tx, git_diff_notifications) = mpsc::unbounded_channel();
        let (auth_tx, auth_methods_updates) = mpsc::unbounded_channel();
        let (elicitation_tx, elicitation_requests) = mpsc::unbounded_channel();
        let (completion_tx, elicitation_completions) = mpsc::unbounded_channel();
        let elicitation_responses = Arc::new(Mutex::new(VecDeque::new()));
        let responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<CreateElicitationResponse>>>>> =
            Arc::new(Mutex::new(None));

        let builder = Client
            .v2()
            .name("test-client")
            .on_receive_notification(
                async move |n: UpdateSessionNotification, _cx| forward(&session_tx, n),
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                async move |n: McpNotification, _cx| forward(&mcp_tx, n),
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                async move |n: GitDiffEventPayload, _cx| forward(&git_diff_tx, n),
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                async move |n: AuthMethodsUpdatedParams, _cx| forward(&auth_tx, n),
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                async move |n: CompleteElicitationNotification, _cx| forward(&completion_tx, n),
                acp::on_receive_notification!(),
            )
            .on_receive_request(
                {
                    let responses = elicitation_responses.clone();
                    let capture = responder_capture.clone();
                    async move |req: CreateElicitationRequest, responder: Responder<CreateElicitationResponse>, _cx| {
                        if let Some(capture_tx) = capture.lock().unwrap().take() {
                            return match capture_tx.send(responder) {
                                Ok(()) => Ok(()),
                                Err(responder) => responder.respond_with_error(acp::Error::internal_error()),
                            };
                        }
                        let _ = elicitation_tx.send(req);
                        let queued = responses.lock().unwrap().pop_front();
                        match queued {
                            Some(response) => responder.respond(response),
                            None => responder.respond_with_error(acp::Error::method_not_found()),
                        }
                    }
                },
                acp::on_receive_request!(),
            );

        let peer = Self {
            session_notifications,
            mcp_notifications,
            git_diff_notifications,
            auth_methods_updates,
            elicitation_requests,
            elicitation_completions,
            elicitation_responses,
            responder_capture,
        };
        (peer, builder)
    }

    pub async fn next_session_notification(&mut self) -> UpdateSessionNotification {
        self.session_notifications.recv().await.expect("peer channel closed")
    }

    pub async fn next_mcp_notification(&mut self) -> McpNotification {
        self.mcp_notifications.recv().await.expect("peer channel closed")
    }

    pub async fn next_git_diff_notification(&mut self) -> GitDiffEventPayload {
        self.git_diff_notifications.recv().await.expect("peer channel closed")
    }

    pub async fn next_auth_methods_update(&mut self) -> AuthMethodsUpdatedParams {
        self.auth_methods_updates.recv().await.expect("peer channel closed")
    }

    pub async fn next_elicitation_request(&mut self) -> CreateElicitationRequest {
        self.elicitation_requests.recv().await.expect("peer channel closed")
    }

    pub async fn next_elicitation_completion(&mut self) -> CompleteElicitationNotification {
        self.elicitation_completions.recv().await.expect("peer channel closed")
    }

    pub fn queue_elicitation_response(&self, response: CreateElicitationResponse) {
        self.elicitation_responses.lock().unwrap().push_back(response);
    }

    pub fn capture_next_elicitation(&self) -> oneshot::Receiver<Responder<CreateElicitationResponse>> {
        let (sender, receiver) = oneshot::channel();
        *self.responder_capture.lock().unwrap() = Some(sender);
        receiver
    }

    pub async fn fake_elicitation(
        &mut self,
        cx: &ConnectionTo<Client>,
    ) -> (Responder<CreateElicitationResponse>, oneshot::Receiver<CreateElicitationResponse>) {
        let responder_rx = self.capture_next_elicitation();

        let (response_tx, response_rx) = oneshot::channel::<CreateElicitationResponse>();
        let cx = cx.clone();
        spawn_local(async move {
            if let Ok(resp) = cx.send_request(placeholder_params()).block_task().await {
                let _ = response_tx.send(resp);
            }
        });

        let responder = responder_rx.await.expect("client handler must capture responder");
        (responder, response_rx)
    }
}

pub fn texts(conversation: &crate::conversation::Conversation) -> Vec<String> {
    conversation.items().iter().filter_map(|item| item.text().map(std::borrow::Cow::into_owned)).collect()
}

pub fn message_chunk(message_id: &str, text: &str) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into(), message_id.to_owned()))
}

pub fn thought_chunk(message_id: &str, text: &str) -> SessionUpdate {
    SessionUpdate::AgentThoughtChunk(ContentChunk::new(text.into(), message_id.to_owned()))
}

pub fn user_message(message_id: &str, content: Vec<ContentBlock>) -> SessionUpdate {
    SessionUpdate::UserMessage(UserMessage::new(message_id.to_owned()).content(content))
}

pub fn plan(plan_id: impl Into<PlanId>, entries: Vec<PlanEntry>) -> SessionUpdate {
    SessionUpdate::PlanUpdate(PlanUpdate::new(PlanUpdateContent::items(plan_id, entries)))
}

pub fn tool_call(id: &str, title: &str, status: ToolCallStatus) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(id.to_owned()).title(title).status(status))
}

pub fn select(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> SessionConfigOption {
    SessionConfigOption::select(id.to_owned(), name.to_owned(), current.to_owned(), choices(values))
}

pub fn choices(values: &[(&str, &str)]) -> Vec<SessionConfigSelectOption> {
    values.iter().map(|&(value, label)| SessionConfigSelectOption::new(value.to_owned(), label.to_owned())).collect()
}

pub fn model_picker(current: &str, choices: impl Into<SessionConfigSelectOptions>) -> SessionConfigOption {
    SessionConfigOption::select("model", "Model", current.to_owned(), choices)
        .meta(ConfigOptionMeta { multi_select: true }.into_meta())
}

pub fn model_choice(value: &str, name: &str, meta: SelectOptionMeta) -> SessionConfigSelectOption {
    SessionConfigSelectOption::new(value.to_owned(), name.to_owned()).meta(meta.into_meta())
}

pub fn option_group(name: &str, options: Vec<SessionConfigSelectOption>) -> SessionConfigSelectGroup {
    SessionConfigSelectGroup::new(name.to_owned(), name.to_owned(), options)
}

pub fn session_preview(session_id: &str, cwd: &str) -> SessionPreviewResponse {
    SessionPreviewResponse {
        session_id: session_id.to_owned(),
        cwd: cwd.into(),
        created_at: "2025-01-01T00:00:00Z".to_owned(),
        model: "claude".to_owned(),
        selected_mode: Some("code".to_owned()),
        transcript: vec![
            SessionPreviewTurn { role: SessionPreviewRole::User, text: "hello".to_owned() },
            SessionPreviewTurn { role: SessionPreviewRole::Assistant, text: "hi there".to_owned() },
        ],
        tool_call_count: 1,
        truncated: false,
    }
}

pub fn prompt_search_hit(prompt: &str, match_start: usize, match_end: usize) -> PromptSearchResult {
    PromptSearchResult {
        session_id: "s1".to_owned(),
        cwd: "/tmp/repo".into(),
        session_created_at: "2026-05-17T00:00:00Z".to_owned(),
        prompt: prompt.to_owned(),
        match_start,
        match_end,
    }
}

pub fn prompt_search_response(query: &str, results: Vec<PromptSearchResult>) -> PromptSearchResponse {
    PromptSearchResponse { query: query.to_owned(), results, truncated: false }
}

/// Build a live `ConnectionTo<Client>` over an in-memory duplex transport with
/// a peer on the other end. Must be called inside a `LocalSet`.
pub async fn test_connection() -> (ConnectionTo<Client>, TestPeer) {
    let (peer, client_builder) = TestPeer::new();
    let agent = Agent.v2().name("test-agent").on_receive_request(
        async |_: InitializeRequest, responder: Responder<InitializeResponse>, _cx| {
            responder.respond(initialize_response())
        },
        acp::on_receive_request!(),
    );
    let pair = connect_pair(agent, client_builder).await;
    pair.client.send_request(initialize_request()).block_task().await.expect("initialize test peers");
    (pair.agent, peer)
}

pub struct ConnectedPair {
    pub agent: ConnectionTo<Client>,
    pub client: ConnectionTo<Agent>,
    pub agent_task: tokio::task::JoinHandle<Result<(), acp::Error>>,
    pub client_task: tokio::task::JoinHandle<Result<(), acp::Error>>,
}

pub async fn connect_pair<T, U, V, X, Y, Z>(
    agent: V2Builder<Agent, T, U, V>,
    client: V2Builder<Client, X, Y, Z>,
) -> ConnectedPair
where
    T: HandleDispatchFrom<Client> + 'static,
    U: RunWithConnectionTo<Client> + 'static,
    V: HandleConnectionClose<Client> + 'static,
    X: HandleDispatchFrom<Agent> + 'static,
    Y: RunWithConnectionTo<Agent> + 'static,
    Z: HandleConnectionClose<Agent> + 'static,
{
    let (agent_transport, client_transport) = Channel::duplex();
    let (agent_tx, agent_rx) = oneshot::channel();
    let (client_tx, client_rx) = oneshot::channel();
    let agent_task =
        spawn_local(async move { agent.with_runner(CaptureConnection(agent_tx)).connect_to(agent_transport).await });
    let client_task =
        spawn_local(async move { client.with_runner(CaptureConnection(client_tx)).connect_to(client_transport).await });
    ConnectedPair {
        agent: agent_rx.await.expect("agent connection"),
        client: client_rx.await.expect("client connection"),
        agent_task,
        client_task,
    }
}

pub struct CaptureConnection<R: acp::Role>(pub oneshot::Sender<ConnectionTo<R>>);

impl<R: acp::Role> RunWithConnectionTo<R> for CaptureConnection<R> {
    async fn run_with_connection_to(self, cx: ConnectionTo<R>) -> Result<(), acp::Error> {
        let _ = self.0.send(cx.clone());
        cx.incoming_closed().await;
        Ok(())
    }
}

/// Initialization request from an in-memory v2 client.
pub fn initialize_request() -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V2, Implementation::new("test-client", "0.0.0"))
}

/// Initialization response from an in-memory v2 agent.
pub fn initialize_response() -> InitializeResponse {
    InitializeResponse::new(ProtocolVersion::V2, Implementation::new("test-agent", "0.0.0"))
}

pub fn running_state() -> StateUpdate {
    StateUpdate::Running(RunningStateUpdate::new())
}

pub fn idle_state(stop_reason: impl IntoOption<StopReason>) -> StateUpdate {
    StateUpdate::Idle(IdleStateUpdate::new().stop_reason(stop_reason))
}

/// A live foreground turn has started.
pub fn running_notification(session_id: impl Into<SessionId>) -> UpdateSessionNotification {
    UpdateSessionNotification::new(session_id, SessionUpdate::StateUpdate(running_state()))
}

/// Foreground work is idle, optionally with a reported stop reason.
pub fn idle_notification(
    session_id: impl Into<SessionId>,
    stop_reason: Option<StopReason>,
) -> UpdateSessionNotification {
    UpdateSessionNotification::new(session_id, SessionUpdate::StateUpdate(idle_state(stop_reason)))
}

pub fn announced(
    parent: impl Into<SessionId>,
    tool_call_id: &str,
    update: SubagentUpdate,
) -> UpdateSessionNotification {
    let update = update.meta(SubAgentMeta::new(tool_call_id).to_meta());
    UpdateSessionNotification::new(parent, SessionUpdate::SubagentUpdate(update))
}

pub fn on_session(session_id: impl Into<SessionId>, update: SessionUpdate) -> crate::client::AcpEvent {
    UpdateSessionNotification::new(session_id, update).into()
}

pub fn sub_agent_tool_update(session_id: impl Into<SessionId>, update: ToolCallUpdate) -> crate::client::AcpEvent {
    on_session(session_id, SessionUpdate::ToolCallUpdate(update))
}

/// Replace the entries of an agent-owned plan.
pub fn plan_notification(
    session_id: impl Into<SessionId>,
    plan_id: impl Into<PlanId>,
    entries: Vec<PlanEntry>,
) -> UpdateSessionNotification {
    UpdateSessionNotification::new(session_id, plan(plan_id, entries))
}

#[expect(clippy::unnecessary_wraps, reason = "notification handlers must return a Result")]
fn forward<T>(tx: &mpsc::UnboundedSender<T>, value: T) -> Result<(), acp::Error> {
    let _ = tx.send(value);
    Ok(())
}

fn placeholder_params() -> CreateElicitationRequest {
    CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("test-session"), ElicitationSchema::new()),
        String::new(),
    )
}
