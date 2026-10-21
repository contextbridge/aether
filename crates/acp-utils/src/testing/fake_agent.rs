use super::{idle_notification, initialize_response, running_notification};
use crate::notifications::{
    AetherCapabilities, PromptSearchParams, PromptSearchResponse, PromptSearchResult, SessionPreviewParams,
    SessionPreviewResponse,
};
use agent_client_protocol::schema::v2::{
    AgentCapabilities, CancelSessionNotification, CloseSessionRequest, CloseSessionResponse, CompactionStatus,
    CompactionUpdate, ContentChunk, CreateElicitationRequest, ElicitationFormMode, ElicitationSchema,
    ElicitationSessionScope, ElicitationUrlMode, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoginAuthRequest, LoginAuthResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ResumeSessionRequest, ResumeSessionResponse, SessionId,
    SessionInfo, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
    UpdateSessionNotification, UserMessage,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{
    self as acp, Agent, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled, NullRun, Responder, V2Builder,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

pub struct FakeAgent {
    initialize: InitializeResponse,
    new_session: Option<NewSessionResponse>,
    sessions: Option<Vec<SessionInfo>>,
    previews: Vec<SessionPreviewResponse>,
    prompt_history: Vec<String>,
    login_method: Option<String>,
    hold_config: bool,
    hold_list_sessions: bool,
    replay: Vec<UpdateSessionNotification>,
    live: Vec<UpdateSessionNotification>,
    turn: Option<Turn>,
    /// How many prompts it has run, which keeps each turn's message ids distinct.
    turns: AtomicUsize,
    capture: Option<Capture>,
}

pub struct FakeAgentRequests {
    pub connection: mpsc::UnboundedReceiver<ConnectionTo<Client>>,
    pub initialize: mpsc::UnboundedReceiver<InitializeRequest>,
    pub new_session: mpsc::UnboundedReceiver<NewSessionRequest>,
    pub login: mpsc::UnboundedReceiver<LoginAuthRequest>,
    pub config: mpsc::UnboundedReceiver<SetSessionConfigOptionRequest>,
    pub prompt: mpsc::UnboundedReceiver<(PromptRequest, Responder<PromptResponse>)>,
    pub resume: mpsc::UnboundedReceiver<(ResumeSessionRequest, Responder<ResumeSessionResponse>)>,
    pub cancel: mpsc::UnboundedReceiver<CancelSessionNotification>,
    pub pending_config: mpsc::UnboundedReceiver<Responder<SetSessionConfigOptionResponse>>,
    pub list_sessions: mpsc::UnboundedReceiver<(ListSessionsRequest, Responder<ListSessionsResponse>)>,
    pub close_session: mpsc::UnboundedReceiver<CloseSessionRequest>,
}

impl Default for FakeAgent {
    fn default() -> Self {
        Self {
            initialize: initialize_response(),
            new_session: None,
            sessions: None,
            previews: Vec::new(),
            prompt_history: Vec::new(),
            login_method: None,
            hold_config: false,
            hold_list_sessions: false,
            replay: Vec::new(),
            live: Vec::new(),
            turn: None,
            turns: AtomicUsize::new(0),
            capture: None,
        }
    }
}

impl FakeAgent {
    pub fn remote_server(mut self, info: &crate::notifications::RemoteServerInfo) -> Self {
        self.initialize.meta = Some(info.to_meta());
        self
    }

    pub fn session_preview(mut self, preview: SessionPreviewResponse) -> Self {
        self.previews.push(preview);
        self
    }

    /// Advertises prompt search and answers it from `prompts`, oldest first: the ones that
    /// contain the query, newest first.
    pub fn prompt_history(mut self, prompts: &[&str]) -> Self {
        let capabilities = AetherCapabilities { prompt_search: true, ..AetherCapabilities::default() };
        let session = self.initialize.capabilities.session.take().unwrap_or_default();
        self.initialize.capabilities.session = Some(session.meta(Some(capabilities.to_meta())));
        self.prompt_history = prompts.iter().map(|prompt| (*prompt).to_owned()).collect();
        self
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
    pub fn hold_config(mut self, hold: bool) -> Self {
        self.hold_config = hold;
        self
    }
    pub fn hold_list_sessions(mut self, hold: bool) -> Self {
        self.hold_list_sessions = hold;
        self
    }
    pub fn new_session_response(mut self, response: NewSessionResponse) -> Self {
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
    pub fn prompt_reply(mut self, text: &str) -> Self {
        self.turn = Some(Turn::Reply(text.into()));
        self
    }

    /// Answers every prompt by sending `updates` in order, then ending the turn.
    pub fn prompt_updates(mut self, updates: Vec<SessionUpdate>) -> Self {
        self.turn = Some(Turn::Updates(updates));
        self
    }

    /// Answers every prompt by asking `message` with no fields, then echoing the answer.
    pub fn prompt_elicitation(self, message: &str) -> Self {
        self.prompt_form(message, ElicitationSchema::new())
    }

    /// Answers every prompt by asking `message` with the form `schema`, then echoing the answer.
    pub fn prompt_form(mut self, message: &str, schema: ElicitationSchema) -> Self {
        self.turn = Some(Turn::Elicit { message: message.into(), question: Question::Form(schema) });
        self
    }

    /// Answers every prompt by asking `message` to be answered at `url`, then echoing the answer.
    pub fn prompt_url(mut self, message: &str, url: &str) -> Self {
        self.turn = Some(Turn::Elicit { message: message.into(), question: Question::Url(url.into()) });
        self
    }
    pub fn compaction(mut self, session_id: &str, compaction_id: &str, status: CompactionStatus) -> Self {
        self.replay.push(UpdateSessionNotification::new(
            session_id,
            SessionUpdate::CompactionUpdate(CompactionUpdate::new(compaction_id, status)),
        ));
        self
    }

    pub fn capture(mut self) -> (Self, FakeAgentRequests) {
        let (connection, connection_rx) = mpsc::unbounded_channel();
        let (initialize, initialize_rx) = mpsc::unbounded_channel();
        let (new_session, new_session_rx) = mpsc::unbounded_channel();
        let (login, login_rx) = mpsc::unbounded_channel();
        let (config, config_rx) = mpsc::unbounded_channel();
        let (prompt, prompt_rx) = mpsc::unbounded_channel();
        let (resume, resume_rx) = mpsc::unbounded_channel();
        let (cancel, cancel_rx) = mpsc::unbounded_channel();
        let (pending_config, pending_config_rx) = mpsc::unbounded_channel();
        let (list_sessions, list_sessions_rx) = mpsc::unbounded_channel();
        let (close_session, close_session_rx) = mpsc::unbounded_channel();
        self.capture = Some(Capture {
            connection,
            initialize,
            new_session,
            login,
            config,
            prompt,
            resume,
            cancel,
            pending_config,
            list_sessions,
            close_session,
        });
        (
            self,
            FakeAgentRequests {
                connection: connection_rx,
                initialize: initialize_rx,
                new_session: new_session_rx,
                login: login_rx,
                config: config_rx,
                prompt: prompt_rx,
                resume: resume_rx,
                cancel: cancel_rx,
                pending_config: pending_config_rx,
                list_sessions: list_sessions_rx,
                close_session: close_session_rx,
            },
        )
    }

    pub fn agent(self) -> V2Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
        Agent.v2().name("fake-agent").with_handler(self)
    }

    pub async fn build(self) -> Result<crate::client::AcpClient, crate::client::AcpClientError> {
        let (agent, client) = super::Channel::duplex();
        tokio::task::spawn_local(self.agent().connect_to(agent));
        crate::client::connect_acp_client(client, super::initialize_request()).await
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
                if let Some(capture) = &self.capture {
                    let _ = capture.connection.send(cx.clone());
                    let _ = capture.initialize.send(request);
                }
                responder.respond(self.initialize.clone())
            })
            .await
            .if_request(async |request: NewSessionRequest, responder| {
                let Some(response) = &self.new_session else {
                    return Ok(Handled::No { message: (request, responder), retry: false });
                };
                if let Some(capture) = &self.capture {
                    let _ = capture.new_session.send(request);
                }
                responder.respond(response.clone())?;
                Ok(Handled::Yes)
            })
            .await
            .if_request(async |request: SessionPreviewParams, responder| {
                let Some(preview) = self.previews.iter().find(|preview| preview.session_id == request.session_id)
                else {
                    return Ok(Handled::No { message: (request, responder), retry: false });
                };
                responder.respond(preview.clone())?;
                Ok(Handled::Yes)
            })
            .await
            .if_request(async |request: PromptSearchParams, responder| responder.respond(self.search_prompts(request)))
            .await
            .if_request(async |request: ListSessionsRequest, responder| {
                if self.hold_list_sessions
                    && let Some(capture) = &self.capture
                {
                    let _ = capture.list_sessions.send((request, responder));
                    return Ok(Handled::Yes);
                }
                let Some(sessions) = &self.sessions else {
                    return Ok(Handled::No { message: (request, responder), retry: false });
                };
                responder.respond(ListSessionsResponse::new(sessions.clone()))?;
                Ok(Handled::Yes)
            })
            .await
            .if_request(async |request: CloseSessionRequest, responder| {
                if let Some(capture) = &self.capture {
                    let _ = capture.close_session.send(request);
                }
                responder.respond(CloseSessionResponse::new())
            })
            .await
            .if_request(async |request: LoginAuthRequest, responder| {
                let allowed = self.login_method.as_deref() == Some(request.method_id.0.as_ref());
                if let Some(capture) = &self.capture {
                    let _ = capture.login.send(request);
                }
                if allowed {
                    responder.respond(LoginAuthResponse::new())
                } else {
                    responder.respond_with_error(acp::Error::invalid_params())
                }
            })
            .await
            .if_request(async |request: SetSessionConfigOptionRequest, responder| {
                if let Some(capture) = &self.capture {
                    let _ = capture.config.send(request);
                    if self.hold_config {
                        let _ = capture.pending_config.send(responder);
                        return Ok(());
                    }
                }
                responder.respond(SetSessionConfigOptionResponse::new(vec![]))
            })
            .await
            .if_request(async |request: PromptRequest, responder| {
                if let Some(capture) = &self.capture {
                    let _ = capture.prompt.send((request, responder));
                    return Ok(());
                }
                self.run_turn(request, responder, &cx)
            })
            .await
            .if_request(async |request: ResumeSessionRequest, responder| self.resume(request, responder, &cx))
            .await
            .if_notification(async |notification: CancelSessionNotification| {
                if let Some(capture) = &self.capture {
                    let _ = capture.cancel.send(notification);
                }
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
    /// The prompts in its history that contain the query, newest first.
    fn search_prompts(&self, request: PromptSearchParams) -> PromptSearchResponse {
        let results = self
            .prompt_history
            .iter()
            .rev()
            .filter_map(|prompt| {
                let match_start = prompt.find(&request.query)?;
                Some(PromptSearchResult {
                    session_id: "history-session".to_owned(),
                    cwd: "/".into(),
                    session_created_at: "2026-01-01T00:00:00Z".to_owned(),
                    prompt: prompt.clone(),
                    match_start,
                    match_end: match_start + request.query.len(),
                })
            })
            .collect();
        PromptSearchResponse { query: request.query, results, truncated: false }
    }

    fn resume(
        &self,
        request: ResumeSessionRequest,
        responder: Responder<ResumeSessionResponse>,
        cx: &ConnectionTo<Client>,
    ) -> Result<(), acp::Error> {
        if let Some(capture) = &self.capture {
            let _ = capture.resume.send((request, responder));
            return Ok(());
        }
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

    fn run_turn(
        &self,
        request: PromptRequest,
        responder: Responder<PromptResponse>,
        cx: &ConnectionTo<Client>,
    ) -> Result<(), acp::Error> {
        let index = self.turns.fetch_add(1, Ordering::Relaxed);
        let user_message_id = format!("user-message-{index}");
        let message_id = format!("message-{index}");
        let session_id = request.session_id;
        let Some(turn) = &self.turn else {
            return responder.respond(PromptResponse::new(user_message_id));
        };
        cx.send_notification(running_notification(session_id.clone()))?;
        let user_message = UserMessage::new(user_message_id.clone()).content(request.prompt);
        cx.send_notification(UpdateSessionNotification::new(
            session_id.clone(),
            SessionUpdate::UserMessage(user_message),
        ))?;
        responder.respond(PromptResponse::new(user_message_id))?;
        match turn {
            Turn::Reply(text) => {
                cx.send_notification(message(&session_id.0, &message_id, text))?;
                cx.send_notification(idle_notification(session_id, Some(StopReason::EndTurn)))
            }
            Turn::Updates(updates) => {
                for update in updates {
                    cx.send_notification(UpdateSessionNotification::new(session_id.clone(), update.clone()))?;
                }
                cx.send_notification(idle_notification(session_id, Some(StopReason::EndTurn)))
            }
            Turn::Elicit { message: prompt, question } => {
                let scope = ElicitationSessionScope::new(session_id.clone());
                let request = match question {
                    Question::Form(schema) => {
                        CreateElicitationRequest::new(ElicitationFormMode::new(scope, schema.clone()), prompt.clone())
                    }
                    Question::Url(url) => CreateElicitationRequest::new(
                        ElicitationUrlMode::new(scope, format!("elicitation-{index}"), url.clone()),
                        prompt.clone(),
                    ),
                };
                let connection = cx.clone();
                cx.spawn(async move {
                    let echo = match connection.send_request(request).block_task().await {
                        Ok(response) => serde_json::to_string(&response),
                        Err(error) => serde_json::to_string(&error),
                    }
                    .map_err(acp::Error::into_internal_error)?;
                    connection.send_notification(message(&session_id.0, &message_id, &echo))?;
                    connection.send_notification(idle_notification(session_id, Some(StopReason::EndTurn)))
                })
            }
        }
    }
}

enum Turn {
    Reply(String),
    Updates(Vec<SessionUpdate>),
    Elicit { message: String, question: Question },
}

enum Question {
    Form(ElicitationSchema),
    Url(String),
}

struct Capture {
    connection: mpsc::UnboundedSender<ConnectionTo<Client>>,
    initialize: mpsc::UnboundedSender<InitializeRequest>,
    new_session: mpsc::UnboundedSender<NewSessionRequest>,
    login: mpsc::UnboundedSender<LoginAuthRequest>,
    config: mpsc::UnboundedSender<SetSessionConfigOptionRequest>,
    prompt: mpsc::UnboundedSender<(PromptRequest, Responder<PromptResponse>)>,
    resume: mpsc::UnboundedSender<(ResumeSessionRequest, Responder<ResumeSessionResponse>)>,
    cancel: mpsc::UnboundedSender<CancelSessionNotification>,
    pending_config: mpsc::UnboundedSender<Responder<SetSessionConfigOptionResponse>>,
    list_sessions: mpsc::UnboundedSender<(ListSessionsRequest, Responder<ListSessionsResponse>)>,
    close_session: mpsc::UnboundedSender<CloseSessionRequest>,
}

/// The message id of every replayed and live message.
const REPLAYED_MESSAGE_ID: &str = "message";

fn message(session_id: &str, message_id: &str, text: &str) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        SessionId::new(session_id),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into(), message_id.to_owned())),
    )
}
