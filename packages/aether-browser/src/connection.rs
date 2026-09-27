use crate::ClientError;
use crate::conversation::TrackedConversation;
use crate::elicitation::Elicitation;
use crate::js::to_js;
use crate::types::AetherClientEvent;
use crate::websocket::CloseStatus;
use acp_utils::client::{AcpClientError, AcpClientHandle, AcpEvent};
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, ContentBlock, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SessionId,
};
use futures::StreamExt;
use futures::future::LocalBoxFuture;
use futures::stream::FuturesUnordered;
use js_sys::{Function, Promise};
use serde::Serialize;
use std::iter;
use tokio::sync::{mpsc, oneshot};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::{future_to_promise, spawn_local};
use web_sys::console;

pub(crate) enum Command {
    NewSession(NewSessionRequest, Reply),
    ResumeSession(ResumeSessionRequest, Reply),
    Prompt(Vec<ContentBlock>, Reply),
    Cancel(Reply),
    CloseSession(Reply),
    Disconnect(Reply),
}

pub(crate) struct Reply(oneshot::Sender<Result<JsValue, ClientError>>);

pub(crate) struct Connection {
    on_event: Function,
    close: CloseStatus,
    current: Option<TrackedConversation>,
}

impl Connection {
    /// Start the connection, returning where to send its commands.
    pub(crate) fn spawn(
        handle: AcpClientHandle,
        events: mpsc::UnboundedReceiver<AcpEvent>,
        on_event: Function,
        close: CloseStatus,
    ) -> mpsc::UnboundedSender<Command> {
        let (commands_tx, commands) = mpsc::unbounded_channel();
        let connection = Self { on_event, close, current: None };
        spawn_local(connection.run(handle, commands, events));
        commands_tx
    }

    async fn run(
        mut self,
        handle: AcpClientHandle,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut events: mpsc::UnboundedReceiver<AcpEvent>,
    ) {
        let mut handle = Some(handle);
        let mut requests = FuturesUnordered::<Pending>::new();
        loop {
            tokio::select! {
                biased;
                Some(event) = events.recv() => self.deliver(event, &mut events),
                Some(settle) = requests.next() => settle(&mut self),
                command = commands.recv(), if handle.is_some() => match command {
                    Some(command) => requests.extend(handle.as_ref().and_then(|handle| self.execute(handle, command))),
                    // Closing the socket settles the requests still in flight and ends the events with
                    // `ConnectionClosed`.
                    None => handle = None,
                },
                else => break,
            }
        }
    }

    fn execute(&mut self, handle: &AcpClientHandle, command: Command) -> Option<Pending> {
        match command {
            Command::NewSession(request, reply) => {
                Some(pending(handle.new_session(request), |connection, response| {
                    if let Ok(response) = &response {
                        connection.open(response.session_id.clone());
                    }
                    reply.respond(response);
                }))
            }
            Command::ResumeSession(request, reply) => {
                let session_id = request.session_id.clone();
                self.open(session_id.clone());
                Some(pending(handle.resume_session(request), move |connection, response| {
                    if response.is_err() {
                        connection.end(&session_id);
                    }
                    reply.respond(response);
                }))
            }
            Command::Prompt(prompt, reply) => match self.start_prompt(prompt.clone()) {
                Ok(session_id) => {
                    let response = handle.prompt(PromptRequest::new(session_id.clone(), prompt));
                    Some(pending(response, move |connection, response| {
                        connection.settle_prompt(&session_id, &response);
                        reply.respond(response);
                    }))
                }
                Err(error) => {
                    reply.send(Err(error));
                    None
                }
            },
            Command::Cancel(reply) => {
                let cancelled = self
                    .current_session()
                    .and_then(|session_id| Ok(handle.cancel(CancelSessionNotification::new(session_id))?));
                reply.send(cancelled.map(|()| JsValue::UNDEFINED));
                None
            }
            Command::CloseSession(reply) => match self.current_session() {
                Ok(session_id) => {
                    let response = handle.request(CloseSessionRequest::new(session_id.clone()));
                    Some(pending(response, move |connection, response| {
                        if response.is_ok() {
                            connection.end(&session_id);
                        }
                        reply.respond(response);
                    }))
                }
                Err(error) => {
                    reply.send(Err(error));
                    None
                }
            },
            Command::Disconnect(reply) => {
                let handle = handle.clone();
                let closed = async move { handle.disconnect().await };
                Some(pending(closed, |_, ()| reply.send(Ok(JsValue::UNDEFINED))))
            }
        }
    }

    fn deliver(&mut self, event: AcpEvent, events: &mut mpsc::UnboundedReceiver<AcpEvent>) {
        for event in iter::once(event).chain(iter::from_fn(|| events.try_recv().ok())) {
            self.reduce(&event);
            self.emit(self.to_js_event(event));
        }
        self.flush();
    }

    fn current_session(&self) -> Result<SessionId, ClientError> {
        self.current.as_ref().map(|tracked| tracked.session_id.clone()).ok_or(ClientError::NoSession)
    }

    fn open(&mut self, session_id: SessionId) {
        self.current = Some(TrackedConversation::new(session_id));
        self.flush();
    }

    fn end(&mut self, session_id: &SessionId) {
        if self.current.take_if(|tracked| tracked.session_id == *session_id).is_some() {
            self.emit(to_js(&AetherClientEvent::ConversationChanged { conversation: None }));
        }
    }

    fn start_prompt(&mut self, content: Vec<ContentBlock>) -> Result<SessionId, ClientError> {
        let tracked = self.current.as_mut().ok_or(ClientError::NoSession)?;
        tracked.conversation.start_prompt(Some(content))?;
        let session_id = tracked.session_id.clone();
        self.flush();
        Ok(session_id)
    }

    fn settle_prompt<T>(&mut self, session_id: &SessionId, response: &Result<T, AcpClientError>) {
        if let Some(tracked) = self.current.as_mut().filter(|tracked| tracked.session_id == *session_id) {
            match response {
                Ok(_) => tracked.conversation.accept_prompt(),
                Err(error) => tracked.conversation.fail_prompt(error),
            }
        }
        self.flush();
    }

    fn reduce(&mut self, event: &AcpEvent) {
        if let Some(tracked) = self.current.as_mut()
            && event.session_id().is_none_or(|session_id| *session_id == tracked.session_id)
        {
            tracked.conversation.apply_event(event);
        }
    }

    fn to_js_event(&self, event: AcpEvent) -> Result<JsValue, ClientError> {
        match event {
            AcpEvent::SessionUpdate(notification) => {
                to_js(&AetherClientEvent::SessionUpdate { notification: &notification })
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                let elicitation = Elicitation::new(&params, responder)?;
                to_js(&AetherClientEvent::ElicitationRequest { elicitation: elicitation.into() })
            }
            AcpEvent::ContextCleared(params) => to_js(&AetherClientEvent::ContextCleared { params: &params }),
            AcpEvent::SubAgentProgress(params) => to_js(&AetherClientEvent::SubAgentProgress { params: &params }),
            AcpEvent::AuthMethodsUpdated(params) => to_js(&AetherClientEvent::AuthMethodsUpdated { params: &params }),
            AcpEvent::McpNotification(params) => to_js(&AetherClientEvent::McpNotification { params: &params }),
            AcpEvent::GitDiffEvent(params) => to_js(&AetherClientEvent::GitDiffEvent { params: &params }),
            AcpEvent::ConnectionClosed => to_js(&AetherClientEvent::ConnectionClosed { close: self.close.get() }),
        }
    }

    fn emit(&self, event: Result<JsValue, ClientError>) {
        match event {
            Ok(event) => {
                if let Err(exception) = self.on_event.call1(&JsValue::NULL, &event) {
                    console::error_2(&"AetherClient: onEvent threw".into(), &exception);
                }
            }
            Err(error) => console::error_1(&format!("AetherClient: dropped an event: {error}").into()),
        }
    }

    fn flush(&mut self) {
        let event = self.current.as_mut().and_then(TrackedConversation::changed).map(|snapshot| {
            snapshot.and_then(|conversation| {
                to_js(&AetherClientEvent::ConversationChanged { conversation: Some(conversation) })
            })
        });
        if let Some(event) = event {
            self.emit(event);
        }
    }
}

impl Reply {
    pub(crate) fn promise() -> (Self, Promise) {
        let (reply, response) = oneshot::channel();
        let settled = async move { response.await.unwrap_or(Err(ClientError::Stopped)).map_err(JsValue::from) };
        (Self(reply), future_to_promise(settled))
    }

    fn send(self, result: Result<JsValue, ClientError>) {
        let _ = self.0.send(result);
    }

    fn respond<T: Serialize>(self, response: Result<T, AcpClientError>) {
        self.send(response.map_err(ClientError::from).and_then(|response| to_js(&response)));
    }
}

/// A request in flight, resolving to what to do with its response.
type Pending = LocalBoxFuture<'static, Box<dyn FnOnce(&mut Connection)>>;

fn pending<T: 'static>(
    response: impl Future<Output = T> + 'static,
    settle: impl FnOnce(&mut Connection, T) + 'static,
) -> Pending {
    Box::pin(async move {
        let response = response.await;
        Box::new(move |connection: &mut Connection| settle(connection, response)) as Box<dyn FnOnce(&mut Connection)>
    })
}
