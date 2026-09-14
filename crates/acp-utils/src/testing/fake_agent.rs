use super::{idle_notification, initialize_response};
use agent_client_protocol::schema::v2::{
    AgentCapabilities, CancelSessionNotification, CloseSessionRequest, CloseSessionResponse, CompactionStatus,
    CompactionUpdate, ContentChunk, Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoginAuthRequest, LoginAuthResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionInfo, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, UpdateSessionNotification,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{
    self as acp, Agent, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled, NullRun, Responder, V2Builder,
};
use tokio::sync::mpsc;

pub struct FakeAgent {
    initialize: InitializeResponse,
    new_session: Option<NewSessionResponse>,
    sessions: Option<Vec<SessionInfo>>,
    login_method: Option<String>,
    hold_config: bool,
    hold_list_sessions: bool,
    replay: Vec<UpdateSessionNotification>,
    live: Vec<UpdateSessionNotification>,
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
            login_method: None,
            hold_config: false,
            hold_list_sessions: false,
            replay: Vec::new(),
            live: Vec::new(),
            capture: None,
        }
    }
}

impl FakeAgent {
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
        self.replay.push(message(session_id, text));
        self
    }
    pub fn live_message(mut self, session_id: &str, text: &str) -> Self {
        self.live.push(message(session_id, text));
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
        let (agent, client) = super::duplex_pair();
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
                } else {
                    responder.respond(PromptResponse::new())?;
                }
                Ok(())
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

fn message(session_id: &str, text: &str) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        SessionId::new(session_id),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into(), "message")),
    )
}
