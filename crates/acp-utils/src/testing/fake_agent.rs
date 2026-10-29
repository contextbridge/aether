use super::{idle_notification, initialize_response, running_notification};
use crate::notifications::{
    AetherCapabilities, AetherMeta, McpRequest, PromptSearchParams, PromptSearchResponse, SessionPreviewParams,
    SessionPreviewResponse, WorkspaceListParams, WorkspaceMoveParams, WorkspaceMoveTarget, WorkspaceStatusPayload,
    WorkspaceStatusResponse,
};
use agent_client_protocol::schema::v2::{
    AgentCapabilities, CancelSessionNotification, CloseSessionRequest, CloseSessionResponse, CompactionStatus,
    CompactionUpdate, CreateElicitationRequest, CreateElicitationResponse, ElicitationFormMode, ElicitationSchema,
    ElicitationSessionScope, ElicitationUrlMode, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoginAuthRequest, LoginAuthResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ResumeSessionRequest, ResumeSessionResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionValue, SessionId, SessionInfo, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason, StringPropertySchema,
    UpdateSessionNotification,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{
    self as acp, Agent, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled, JsonRpcRequest, NullRun,
    Responder, V2Builder,
};
use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use utils::artifact_review::{ArtifactReviewDecision, ArtifactReviewElicitationMeta};

pub struct FakeAgent {
    initialize: InitializeResponse,
    new_session: Option<NewSessionResponse>,
    sessions: Option<Vec<SessionInfo>>,
    previews: Vec<SessionPreviewResponse>,
    prompt_history: Vec<String>,
    login_method: Option<String>,
    config_options: Vec<SessionConfigOption>,
    workspace_statuses: HashMap<String, WorkspaceStatusResponse>,
    session_cwds: HashMap<String, std::path::PathBuf>,
    holds: Holds,
    queue: QueuedTurns,
    replay: Vec<UpdateSessionNotification>,
    live: Vec<UpdateSessionNotification>,
    recorder: Recorder,
    requests: Option<FakeAgentRequests>,
}

pub struct FakeAgentRequests {
    pub connection: mpsc::UnboundedReceiver<ConnectionTo<Client>>,
    pub initialize: mpsc::UnboundedReceiver<InitializeRequest>,
    pub new_session: mpsc::UnboundedReceiver<NewSessionRequest>,
    pub login: mpsc::UnboundedReceiver<LoginAuthRequest>,
    pub config: mpsc::UnboundedReceiver<SetSessionConfigOptionRequest>,
    pub prompt: mpsc::UnboundedReceiver<PromptRequest>,
    pub resume: mpsc::UnboundedReceiver<ResumeSessionRequest>,
    pub cancel: mpsc::UnboundedReceiver<CancelSessionNotification>,
    pub mcp: mpsc::UnboundedReceiver<McpRequest>,
    pub search: mpsc::UnboundedReceiver<PromptSearchParams>,
    pub close_session: mpsc::UnboundedReceiver<CloseSessionRequest>,
    pub turns: QueuedTurns,
    held: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

pub enum Question {
    Form(ElicitationSchema),
    Url(String),
    ArtifactReview(ArtifactReviewElicitationMeta),
}

pub type HeldRequests<T> = mpsc::UnboundedReceiver<(T, Responder<<T as JsonRpcRequest>::Response>)>;

#[derive(Clone, Default)]
pub struct QueuedTurns(Arc<Mutex<TurnQueue>>);

impl Default for FakeAgent {
    fn default() -> Self {
        let queue = QueuedTurns::default();
        let (recorder, requests) = Recorder::new(queue.clone());
        Self {
            initialize: initialize_response(),
            new_session: None,
            sessions: None,
            previews: Vec::new(),
            prompt_history: Vec::new(),
            login_method: None,
            config_options: Vec::new(),
            workspace_statuses: HashMap::new(),
            session_cwds: HashMap::new(),
            holds: Holds::default(),
            queue,
            replay: Vec::new(),
            live: Vec::new(),
            recorder,
            requests: Some(requests),
        }
    }
}

impl FakeAgent {
    pub fn new(session_id: &str) -> Self {
        Self::default().new_session_response(NewSessionResponse::new(session_id))
    }

    pub fn remote_server(mut self, info: &crate::notifications::RemoteServerInfo) -> Self {
        self.initialize.meta = Some(info.to_meta());
        self
    }

    pub fn session_preview(mut self, preview: SessionPreviewResponse) -> Self {
        self.previews.push(preview);
        self
    }

    pub fn prompt_history(mut self, prompts: &[&str]) -> Self {
        self.prompt_history = prompts.iter().map(|prompt| (*prompt).to_owned()).collect();
        self.advertise(|capabilities| capabilities.prompt_search = true)
    }

    pub fn git_diff(self) -> Self {
        self.advertise(|capabilities| capabilities.git_diff = true)
    }

    pub fn agent_info(mut self, info: Implementation) -> Self {
        self.initialize.info = info;
        self
    }
    pub fn capabilities(mut self, capabilities: AgentCapabilities) -> Self {
        self.initialize.capabilities = capabilities;
        self
    }
    pub fn login_method(mut self, method: &str) -> Self {
        self.login_method = Some(method.into());
        self
    }
    pub fn hold<T: JsonRpcRequest>(mut self) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel::<(T, Responder<T::Response>)>();
        self.holds.0.insert(TypeId::of::<T>(), Box::new(sender));
        let requests = self.requests.as_mut().expect("an agent holds requests before it is captured");
        requests.held.insert(TypeId::of::<T>(), Box::new(receiver));
        self
    }

    pub fn workspace_status(mut self, session_id: &str, status: WorkspaceStatusResponse) -> Self {
        self.workspace_statuses.insert(session_id.to_owned(), status);
        self
    }

    pub fn new_session_response(mut self, response: NewSessionResponse) -> Self {
        if !response.config_options.is_empty() {
            self.config_options.clone_from(&response.config_options);
        }
        self.new_session = Some(response);
        self
    }
    pub fn sessions(mut self, sessions: Vec<SessionInfo>) -> Self {
        self.sessions = Some(sessions);
        self
    }
    pub fn replay_message(mut self, session_id: &str, text: &str) -> Self {
        self.replay.push(message(session_id, REPLAYED_MESSAGE_ID, text));
        self
    }
    pub fn live_message(mut self, session_id: &str, text: &str) -> Self {
        self.live.push(message(session_id, REPLAYED_MESSAGE_ID, text));
        self
    }
    pub fn prompt_reply(self, text: &str) -> Self {
        self.script(Turn::Reply(text.into()))
    }

    pub fn prompt_updates(self, updates: Vec<SessionUpdate>) -> Self {
        self.script(Turn::Updates(updates))
    }

    pub fn prompt_asks(self, message: &str, question: Question) -> Self {
        self.script(Turn::Elicit { message: message.into(), question })
    }
    pub fn compaction(mut self, session_id: &str, compaction_id: &str, status: CompactionStatus) -> Self {
        self.replay.push(UpdateSessionNotification::new(
            session_id,
            SessionUpdate::CompactionUpdate(CompactionUpdate::new(compaction_id, status)),
        ));
        self
    }

    pub fn capture(mut self) -> (Self, FakeAgentRequests) {
        let requests = self.requests.take().expect("an agent is captured once");
        (self, requests)
    }

    pub fn agent(self) -> V2Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
        Agent.v2().name("fake-agent").with_handler(self)
    }

    pub fn serve(self) -> super::Channel {
        let (agent, client) = super::Channel::duplex();
        tokio::task::spawn_local(self.agent().connect_to(agent));
        client
    }

    pub async fn build(self) -> Result<crate::client::AcpClient, crate::client::AcpClientError> {
        crate::client::connect_acp_client(self.serve(), super::initialize_request()).await
    }
}

impl FakeAgentRequests {
    pub fn held<T: JsonRpcRequest>(&mut self) -> &mut HeldRequests<T> {
        self.held
            .get_mut(&TypeId::of::<T>())
            .and_then(|held| held.downcast_mut())
            .unwrap_or_else(|| panic!("the agent does not hold {}", std::any::type_name::<T>()))
    }
}

impl QueuedTurns {
    pub fn update(&self, update: SessionUpdate) {
        let queue = self.0.lock().unwrap();
        let running = queue.running.as_ref().expect("a turn runs");
        queue.send(UpdateSessionNotification::new(running.session_id.clone(), update));
    }

    pub fn reply(&self, text: &str) {
        let queue = self.0.lock().unwrap();
        let running = queue.running.as_ref().expect("a turn runs");
        queue.send(message(&running.session_id.0, &running.message_id, text));
    }

    pub fn end(&self, stop_reason: StopReason) {
        let started = {
            let mut queue = self.0.lock().unwrap();
            let Some(running) = queue.running.take() else {
                return;
            };
            queue.send(idle_notification(running.session_id, Some(stop_reason)));
            let waiting = std::mem::take(&mut queue.waiting);
            let started = !waiting.is_empty();
            for (request, responder) in waiting {
                queue.start(request, responder);
            }
            started
        };
        if started {
            self.play();
        }
    }

    pub fn insert_waiting(&self) {
        let mut queue = self.0.lock().unwrap();
        for (request, responder) in std::mem::take(&mut queue.waiting) {
            queue.take(request, responder);
        }
    }

    pub fn reject_waiting(&self, error: &acp::Error) {
        for (_, responder) in std::mem::take(&mut self.0.lock().unwrap().waiting) {
            let _ = responder.respond_with_error(error.clone());
        }
    }

    pub fn refuse(&self, error: acp::Error) {
        self.0.lock().unwrap().refusal = Some(error);
    }

    pub fn ask(
        &self,
        request: CreateElicitationRequest,
    ) -> oneshot::Receiver<Result<CreateElicitationResponse, acp::Error>> {
        let (answer, answered) = oneshot::channel();
        let cx = self.0.lock().unwrap().cx.clone().expect("the client connected");
        let connection = cx.clone();
        let _ = cx.spawn(async move {
            let _ = answer.send(connection.send_request(request).block_task().await);
            Ok(())
        });
        answered
    }

    fn connect(&self, cx: ConnectionTo<Client>) {
        self.0.lock().unwrap().cx = Some(cx);
    }

    fn receive(&self, request: PromptRequest, responder: Responder<PromptResponse>) -> Result<(), acp::Error> {
        {
            let mut queue = self.0.lock().unwrap();
            if let Some(error) = queue.refusal.clone() {
                return responder.respond_with_error(error);
            }
            if queue.running.is_some() {
                queue.waiting.push_back((request, responder));
                return Ok(());
            }
            queue.start(request, responder);
        }
        self.play();
        Ok(())
    }

    fn play(&self) {
        let Some(script) = self.0.lock().unwrap().script.clone() else {
            return;
        };
        match &*script {
            Turn::Reply(text) => {
                self.reply(text);
                self.end(StopReason::EndTurn);
            }
            Turn::Updates(updates) => {
                for update in updates {
                    self.update(update.clone());
                }
                self.end(StopReason::EndTurn);
            }
            Turn::Elicit { message, question } => {
                let (request, cx) = {
                    let queue = self.0.lock().unwrap();
                    let running = queue.running.as_ref().expect("a turn runs");
                    let scope = ElicitationSessionScope::new(running.session_id.clone());
                    let request = question.request(scope, format!("elicitation-{}", queue.taken), message);
                    (request, queue.cx.clone().expect("the client connected"))
                };
                let answered = self.ask(request);
                let turns = self.clone();
                let _ = cx.spawn(async move {
                    let Ok(answer) = answered.await else {
                        return Ok(());
                    };
                    let echo = match answer {
                        Ok(response) => serde_json::to_string(&response),
                        Err(error) => serde_json::to_string(&error),
                    }
                    .map_err(acp::Error::into_internal_error)?;
                    turns.reply(&echo);
                    turns.end(StopReason::EndTurn);
                    Ok(())
                });
            }
        }
    }
}

impl HandleDispatchFrom<Client> for FakeAgent {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<Client>,
    ) -> Result<Handled<Dispatch>, acp::Error> {
        MatchDispatchFrom::new(message, &cx)
            .if_request(async |request: InitializeRequest, responder| {
                self.queue.connect(cx.clone());
                let _ = self.recorder.connection.send(cx.clone());
                let _ = self.recorder.initialize.send(request);
                responder.respond(self.initialize.clone())
            })
            .await
            .if_request(async |request: NewSessionRequest, responder| {
                let _ = self.recorder.new_session.send(request.clone());
                self.answer_scripted(request, responder, |_| self.new_session.clone())
            })
            .await
            .if_request(async |request: SessionPreviewParams, responder| {
                self.answer_scripted(request, responder, |request| {
                    self.previews.iter().find(|preview| preview.session_id == request.session_id).cloned()
                })
            })
            .await
            .if_request(async |request: PromptSearchParams, responder| {
                let _ = self.recorder.search.send(request.clone());
                self.answer_scripted(request, responder, |request| Some(self.search_prompts(request)))
            })
            .await
            .if_request(async |request: WorkspaceListParams, responder| {
                self.answer_scripted(request, responder, |_| None)
            })
            .await
            .if_request(async |request: WorkspaceMoveParams, responder| {
                if let WorkspaceMoveTarget::Existing { path } = &request.target {
                    self.session_cwds.insert(request.session_id.clone(), path.clone());
                }
                self.answer_scripted(request, responder, |_| None)
            })
            .await
            .if_request(async |request: ListSessionsRequest, responder| {
                self.answer_scripted(request, responder, |_| self.sessions.clone().map(ListSessionsResponse::new))
            })
            .await
            .if_request(async |request: CloseSessionRequest, responder| {
                let _ = self.recorder.close_session.send(request);
                responder.respond(CloseSessionResponse::new())
            })
            .await
            .if_request(async |request: WorkspaceStatusPayload, responder| {
                self.answer_scripted(request, responder, |request| self.workspace_status_of(&request.session_id))
            })
            .await
            .if_request(async |request: LoginAuthRequest, responder| {
                let _ = self.recorder.login.send(request.clone());
                let Some((request, responder)) = self.holds.hand_over(request, responder) else {
                    return Ok(());
                };
                if self.login_method.as_deref() == Some(request.method_id.0.as_ref()) {
                    responder.respond(LoginAuthResponse::new())
                } else {
                    responder.respond_with_error(acp::Error::invalid_params())
                }
            })
            .await
            .if_request(async |request: SetSessionConfigOptionRequest, responder| {
                self.set_config_option(request, responder)
            })
            .await
            .if_request(async |request: PromptRequest, responder| self.take_prompt(request, responder))
            .await
            .if_request(async |request: ResumeSessionRequest, responder| self.resume(request, responder, &cx))
            .await
            .if_notification(async |notification: CancelSessionNotification| {
                let _ = self.recorder.cancel.send(notification);
                Ok(())
            })
            .await
            .if_notification(async |notification: McpRequest| {
                let _ = self.recorder.mcp.send(notification);
                Ok(())
            })
            .await
            .done()
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "FakeAgent"
    }
}

impl FakeAgent {
    fn advertise(mut self, enable: impl FnOnce(&mut AetherCapabilities)) -> Self {
        let mut capabilities = AetherCapabilities::of(&self.initialize.capabilities);
        enable(&mut capabilities);
        let session = self.initialize.capabilities.session.take().unwrap_or_default();
        self.initialize.capabilities.session = Some(session.meta(Some(capabilities.to_meta())));
        self
    }

    fn answer_scripted<T: JsonRpcRequest>(
        &self,
        request: T,
        responder: Responder<T::Response>,
        scripted: impl FnOnce(&T) -> Option<T::Response>,
    ) -> Answered<T> {
        let Some((request, responder)) = self.holds.hand_over(request, responder) else {
            return Ok(Handled::Yes);
        };
        match scripted(&request) {
            Some(response) => {
                responder.respond(response)?;
                Ok(Handled::Yes)
            }
            None => Ok(Handled::No { message: (request, responder), retry: false }),
        }
    }

    fn workspace_status_of(&self, session_id: &str) -> Option<WorkspaceStatusResponse> {
        match self.session_cwds.get(session_id) {
            Some(cwd) => Some(WorkspaceStatusResponse { display_dir: cwd.display().to_string(), git_ref: None }),
            None => self.workspace_statuses.get(session_id).cloned(),
        }
    }

    fn set_config_option(
        &mut self,
        request: SetSessionConfigOptionRequest,
        responder: Responder<SetSessionConfigOptionResponse>,
    ) -> Result<(), acp::Error> {
        let _ = self.recorder.config.send(request.clone());
        let Some((request, responder)) = self.holds.hand_over(request, responder) else {
            return Ok(());
        };
        let option = self.config_options.iter_mut().find(|option| option.config_id == request.config_id);
        if let Some(SessionConfigOption { kind: SessionConfigKind::Select(select), .. }) = option
            && let SessionConfigOptionValue::Id { value } = request.value
        {
            select.current_value = value;
        }
        responder.respond(SetSessionConfigOptionResponse::new(self.config_options.clone()))
    }

    fn script(self, turn: Turn) -> Self {
        self.queue.0.lock().unwrap().script = Some(Arc::new(turn));
        self
    }

    fn take_prompt(&self, request: PromptRequest, responder: Responder<PromptResponse>) -> Result<(), acp::Error> {
        let _ = self.recorder.prompt.send(request.clone());
        let Some((request, responder)) = self.holds.hand_over(request, responder) else {
            return Ok(());
        };
        self.queue.receive(request, responder)
    }

    fn search_prompts(&self, request: &PromptSearchParams) -> PromptSearchResponse {
        let results = self
            .prompt_history
            .iter()
            .rev()
            .filter_map(|prompt| {
                let match_start = prompt.find(&request.query)?;
                Some(super::prompt_search_hit(prompt, match_start, match_start + request.query.len()))
            })
            .collect();
        super::prompt_search_response(&request.query, results)
    }

    fn resume(
        &mut self,
        request: ResumeSessionRequest,
        responder: Responder<ResumeSessionResponse>,
        cx: &ConnectionTo<Client>,
    ) -> Result<(), acp::Error> {
        let _ = self.recorder.resume.send(request.clone());
        self.session_cwds.insert(request.session_id.to_string(), request.cwd.0.clone());
        let Some((request, responder)) = self.holds.hand_over(request, responder) else {
            return Ok(());
        };
        if request.replay_from.is_none() {
            return responder.respond(ResumeSessionResponse::new());
        }
        for notification in &self.replay {
            cx.send_notification(notification.clone())?;
        }
        cx.send_notification(idle_notification(request.session_id, None))?;
        responder.respond(ResumeSessionResponse::new())?;
        for notification in &self.live {
            cx.send_notification(notification.clone())?;
        }
        Ok(())
    }
}

type Answered<T> = Result<Handled<(T, Responder<<T as JsonRpcRequest>::Response>)>, acp::Error>;

enum Turn {
    Reply(String),
    Updates(Vec<SessionUpdate>),
    Elicit { message: String, question: Question },
}

impl Question {
    fn request(&self, scope: ElicitationSessionScope, id: String, message: &str) -> CreateElicitationRequest {
        match self {
            Self::Form(schema) => {
                CreateElicitationRequest::new(ElicitationFormMode::new(scope, schema.clone()), message)
            }
            Self::Url(url) => CreateElicitationRequest::new(ElicitationUrlMode::new(scope, id, url.clone()), message),
            Self::ArtifactReview(meta) => {
                CreateElicitationRequest::new(ElicitationFormMode::new(scope, review_schema()), message)
                    .meta(meta.to_meta())
            }
        }
    }
}

fn review_schema() -> ElicitationSchema {
    let decisions = ArtifactReviewDecision::ALL.map(|decision| decision.to_string()).to_vec();
    ElicitationSchema::new().property("decision", StringPropertySchema::new().enum_values(decisions), true).property(
        "feedback",
        StringPropertySchema::new(),
        false,
    )
}

#[derive(Default)]
struct TurnQueue {
    cx: Option<ConnectionTo<Client>>,
    running: Option<RunningTurn>,
    waiting: VecDeque<(PromptRequest, Responder<PromptResponse>)>,
    refusal: Option<acp::Error>,
    taken: usize,
    script: Option<Arc<Turn>>,
}

struct RunningTurn {
    session_id: SessionId,
    message_id: String,
}

impl TurnQueue {
    fn start(&mut self, request: PromptRequest, responder: Responder<PromptResponse>) {
        if self.running.is_none() {
            self.send(running_notification(request.session_id.clone()));
            let message_id = format!("message-{}", self.taken + 1);
            self.running = Some(RunningTurn { session_id: request.session_id.clone(), message_id });
        }
        self.take(request, responder);
    }

    fn take(&mut self, request: PromptRequest, responder: Responder<PromptResponse>) {
        self.taken += 1;
        let user_message_id = format!("user-message-{}", self.taken);
        let echo = super::user_message(&user_message_id, request.prompt);
        self.send(UpdateSessionNotification::new(request.session_id, echo));
        let _ = responder.respond(PromptResponse::new(user_message_id));
    }

    fn send(&self, notification: UpdateSessionNotification) {
        if let Some(cx) = &self.cx {
            let _ = cx.send_notification(notification);
        }
    }
}

#[derive(Default)]
struct Holds(HashMap<TypeId, Box<dyn Any + Send + Sync>>);

impl Holds {
    fn hand_over<T: JsonRpcRequest>(
        &self,
        request: T,
        responder: Responder<T::Response>,
    ) -> Option<(T, Responder<T::Response>)> {
        let held = self
            .0
            .get(&TypeId::of::<T>())
            .and_then(|held| held.downcast_ref::<mpsc::UnboundedSender<(T, Responder<T::Response>)>>());
        match held {
            Some(sender) => {
                let _ = sender.send((request, responder));
                None
            }
            None => Some((request, responder)),
        }
    }
}

struct Recorder {
    connection: mpsc::UnboundedSender<ConnectionTo<Client>>,
    initialize: mpsc::UnboundedSender<InitializeRequest>,
    new_session: mpsc::UnboundedSender<NewSessionRequest>,
    login: mpsc::UnboundedSender<LoginAuthRequest>,
    config: mpsc::UnboundedSender<SetSessionConfigOptionRequest>,
    prompt: mpsc::UnboundedSender<PromptRequest>,
    resume: mpsc::UnboundedSender<ResumeSessionRequest>,
    cancel: mpsc::UnboundedSender<CancelSessionNotification>,
    mcp: mpsc::UnboundedSender<McpRequest>,
    search: mpsc::UnboundedSender<PromptSearchParams>,
    close_session: mpsc::UnboundedSender<CloseSessionRequest>,
}

impl Recorder {
    fn new(turns: QueuedTurns) -> (Self, FakeAgentRequests) {
        let (connection, connection_rx) = mpsc::unbounded_channel();
        let (initialize, initialize_rx) = mpsc::unbounded_channel();
        let (new_session, new_session_rx) = mpsc::unbounded_channel();
        let (login, login_rx) = mpsc::unbounded_channel();
        let (config, config_rx) = mpsc::unbounded_channel();
        let (prompt, prompt_rx) = mpsc::unbounded_channel();
        let (resume, resume_rx) = mpsc::unbounded_channel();
        let (cancel, cancel_rx) = mpsc::unbounded_channel();
        let (mcp, mcp_rx) = mpsc::unbounded_channel();
        let (search, search_rx) = mpsc::unbounded_channel();
        let (close_session, close_session_rx) = mpsc::unbounded_channel();
        let recorder = Self {
            connection,
            initialize,
            new_session,
            login,
            config,
            prompt,
            resume,
            cancel,
            mcp,
            search,
            close_session,
        };
        let requests = FakeAgentRequests {
            connection: connection_rx,
            initialize: initialize_rx,
            new_session: new_session_rx,
            login: login_rx,
            config: config_rx,
            prompt: prompt_rx,
            resume: resume_rx,
            cancel: cancel_rx,
            mcp: mcp_rx,
            search: search_rx,
            close_session: close_session_rx,
            turns,
            held: HashMap::new(),
        };
        (recorder, requests)
    }
}

const REPLAYED_MESSAGE_ID: &str = "message";

fn message(session_id: &str, message_id: &str, text: &str) -> UpdateSessionNotification {
    UpdateSessionNotification::new(SessionId::new(session_id), super::message_chunk(message_id, text))
}
