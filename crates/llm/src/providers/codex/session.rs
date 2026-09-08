use std::future::Future;
use std::sync::{Arc, OnceLock};

use async_openai::types::responses::{CreateResponse, InputParam, Status};
use futures::{Stream, StreamExt, future::BoxFuture};
use reqwest::{Client, Url, header::HeaderMap};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use uuid::Uuid;

use super::continuation::{ContinuationCheckpoint, DeliveredResponse};
use super::oauth::CodexTokenManager;
use super::websocket::Connection;
use crate::provider::LlmResponseStream;
use crate::providers::openai_responses::streaming::{
    ResponsesCompleted, ResponsesStreamEvent, process_response_stream,
};
use crate::{LlmError, LlmModel, LlmResponse, ProviderError, ProviderErrorKind, ReasoningEffort, Result};

pub(super) struct SessionHandle {
    client: Arc<SessionClient>,
    request_tx: OnceLock<mpsc::Sender<Request>>,
    id: OnceLock<String>,
}

pub(super) struct SessionClient {
    pub client: Client,
    pub url: Url,
    pub model: Option<LlmModel>,
    pub token_manager: Arc<CodexTokenManager>,
}

pub(super) struct InferenceRequest {
    pub headers: HeaderMap,
    pub full: CreateResponse,
    pub effort: Option<ReasoningEffort>,
    pub identity: Credentials,
    pub turn_id: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Credentials {
    pub access_token: String,
    pub account_id: String,
}

impl SessionHandle {
    pub fn new(client: SessionClient) -> Self {
        Self { client: Arc::new(client), request_tx: OnceLock::new(), id: OnceLock::new() }
    }

    pub fn endpoint(&self) -> &SessionClient {
        &self.client
    }

    pub fn id(&self, affinity: Option<&str>) -> Result<&str> {
        let affinity = affinity.filter(|key| !key.is_empty());
        let id = self.id.get_or_init(|| affinity.map_or_else(|| Uuid::new_v4().to_string(), str::to_owned));
        if affinity.is_some_and(|key| key != id) {
            return Err(LlmError::InvalidArgument(
                "CodexProvider already belongs to another conversation; construct a new provider".into(),
            ));
        }
        Ok(id)
    }

    pub fn stream(
        self: &Arc<Self>,
        prepare: impl Future<Output = Result<InferenceRequest>> + Send + 'static,
    ) -> LlmResponseStream {
        let handle = Arc::clone(self);
        Box::pin(async_stream::try_stream! {
            let (responses, mut receiver) = mpsc::channel(1);
            let (finished, acknowledged) = oneshot::channel();
            let requests = handle.request_tx.get_or_init(|| {
                let (sender, receiver) = mpsc::channel(1);
                tokio::spawn(Session::default().run(Arc::clone(&handle.client), receiver));
                sender
            });
            requests.send(Request { prepare: Box::pin(prepare), responses, acknowledged }).await
                .map_err(|_| ProviderError::stream_interrupted("Codex session task stopped"))?;
            while let Some(response) = receiver.recv().await {
                if response.is_err() || matches!(response, Ok(LlmResponse::Done { .. })) {
                    let _ = finished.send(());
                    yield response?;
                    return;
                }
                yield response?;
            }
            Err(ProviderError::stream_interrupted("Codex session task stopped"))?;
        })
    }
}

struct Request {
    prepare: BoxFuture<'static, Result<InferenceRequest>>,
    responses: mpsc::Sender<Result<LlmResponse>>,
    acknowledged: oneshot::Receiver<()>,
}

#[derive(Default)]
struct Session {
    link: Option<Link>,
    identity: Option<Credentials>,
    turn: Turn,
}

/// A live socket and the server-side response lineage it carries.
struct Link {
    connection: Connection,
    checkpoint: Option<ContinuationCheckpoint>,
}

/// The user turn being served and the routing token the server issued for it.
#[derive(Default)]
struct Turn {
    id: String,
    routing_token: Option<String>,
}

impl Session {
    async fn run(mut self, endpoint: Arc<SessionClient>, mut requests: mpsc::Receiver<Request>) {
        loop {
            let request = tokio::select! {
                biased;
                request = requests.recv() => match request {
                    Some(request) => request,
                    None => return,
                },
                () = self.idle(), if self.link.is_some() => continue,
            };
            let Request { prepare, responses, acknowledged } = request;
            if responses.is_closed() {
                continue;
            }
            let outcome = tokio::select! {
                biased;
                () = responses.closed() => {
                    self.link = None;
                    continue;
                }
                outcome = self.infer(&endpoint, prepare, &responses) => outcome,
            };
            if self.link.as_ref().is_some_and(|link| !link.connection.reusable()) {
                self.link = None;
            }
            // Queuing Done is not delivery: only its consumption permits socket reuse.
            if responses.send(outcome).await.is_err() || acknowledged.await.is_err() {
                self.link = None;
            }
        }
    }

    async fn infer(
        &mut self,
        endpoint: &SessionClient,
        prepare: BoxFuture<'static, Result<InferenceRequest>>,
        responses: &mpsc::Sender<Result<LlmResponse>>,
    ) -> Result<LlmResponse> {
        let exchange = prepare.await?;
        self.begin(&exchange);
        let mut delivered = DeliveredResponse::default();
        let mut completed = None;
        let outcome = {
            let events = std::pin::pin!(self.events(endpoint, &exchange, &mut completed));
            let mut stream = std::pin::pin!(process_response_stream(events));
            loop {
                match stream.next().await {
                    Some(Ok(done @ LlmResponse::Done { .. })) => break Ok(done),
                    Some(Ok(response)) => {
                        delivered.observe(&response);
                        responses
                            .send(Ok(response))
                            .await
                            .map_err(|_| ProviderError::stream_interrupted("Codex response consumer dropped"))?;
                    }
                    Some(Err(error)) => break Err(error),
                    None => break Err(ProviderError::stream_interrupted("Codex stream ended without Done").into()),
                }
            }
        };
        self.finish(endpoint, exchange, outcome, completed, &delivered).await
    }

    fn begin(&mut self, exchange: &InferenceRequest) {
        let same_identity = self.identity.as_ref() == Some(&exchange.identity);
        if !same_identity || self.turn.id != exchange.turn_id {
            self.turn = Turn { id: exchange.turn_id.clone(), routing_token: None };
        }
        if !same_identity || self.link.as_ref().is_some_and(|link| !link.connection.reusable()) {
            self.link = None;
        }
        if !same_identity {
            self.identity = Some(exchange.identity.clone());
        }
    }

    fn events<'a>(
        &'a mut self,
        endpoint: &'a SessionClient,
        exchange: &'a InferenceRequest,
        completed: &'a mut Option<ResponsesCompleted>,
    ) -> impl Stream<Item = Result<ResponsesStreamEvent>> + Send + 'a {
        async_stream::try_stream! {
            self.send(endpoint, exchange).await?;
            let mut recovered = false;
            let mut saw_event = false;
            let mut created_id = None;
            loop {
                let link = self.link.as_mut().expect("sent request has a connection");
                let event = match link.connection.receive().await?.into_result() {
                    Ok(event) => event,
                    Err(mut error) => {
                        error.request_id = error.request_id.or_else(|| link.connection.request_id().map(str::to_owned));
                        let recoverable = recovery_code(error.code.as_deref());
                        if recoverable && !saw_event && !recovered {
                            tracing::debug!(reason = error.code.as_deref(), "Codex WebSocket full-context recovery");
                            self.link = None;
                            recovered = true;
                            self.send(endpoint, exchange).await?;
                            continue;
                        }
                        if recoverable {
                            error.kind = ProviderErrorKind::StreamInterrupted;
                        }
                        Err(error)?
                    }
                };
                match &event {
                    ResponsesStreamEvent::Metadata(metadata) => {
                        self.capture_token(metadata.headers.get("x-codex-turn-state"));
                        continue;
                    }
                    ResponsesStreamEvent::Created(created)
                        if created_id.replace(created.response.id.clone()).is_some() =>
                    {
                        Err(ProviderError::stream_interrupted("Duplicate Codex response.created"))?;
                    }
                    ResponsesStreamEvent::Completed(event)
                        if matches!(event.response.status, Some(Status::Completed)) =>
                    {
                        let mut response = event.response.clone();
                        response.id = response.id.filter(|id| Some(id.as_str()) == created_id.as_deref());
                        *completed = Some(response);
                    }
                    _ => {}
                }
                saw_event = true;
                let terminal = event.ends_response();
                yield event;
                if terminal {
                    return;
                }
            }
        }
    }

    async fn send(&mut self, endpoint: &SessionClient, exchange: &InferenceRequest) -> Result<()> {
        let started = Instant::now();
        let reused = self.link.is_some();
        if self.link.is_none() {
            let (connection, token) =
                Connection::open(&endpoint.client, endpoint.url.clone(), exchange.headers.clone()).await?;
            self.capture_token(token.as_deref());
            self.link = Some(Link { connection, checkpoint: None });
        }
        let link = self.link.as_mut().expect("connection opened above");
        tracing::debug!(reused, connect_ms = started.elapsed().as_millis(), "Codex WebSocket connection ready");
        let mut body = exchange.full.clone();
        if let Some(checkpoint) = &link.checkpoint {
            body = checkpoint.prepare(body, exchange.effort);
        }
        tracing::debug!(
            transport = "websocket",
            continuation = body.previous_response_id.is_some(),
            sent_items = match &body.input {
                InputParam::Items(items) => items.len(),
                InputParam::Text(_) => 1,
            },
            "Codex response.create"
        );
        link.connection.send(&body, exchange.effort, &self.turn.id, self.turn.routing_token.as_deref()).await
    }

    async fn finish(
        &mut self,
        endpoint: &SessionClient,
        exchange: InferenceRequest,
        outcome: Result<LlmResponse>,
        completed: Option<ResponsesCompleted>,
        delivered: &DeliveredResponse,
    ) -> Result<LlmResponse> {
        let authentication_failed = outcome
            .as_ref()
            .err()
            .and_then(LlmError::provider)
            .is_some_and(|error| error.kind == ProviderErrorKind::Authentication);
        if authentication_failed {
            self.turn.routing_token = None;
            endpoint.token_manager.clear_cache().await;
        }
        if outcome.is_ok()
            && let Some(completed) = completed
            && let Some(link) = &mut self.link
        {
            link.checkpoint = ContinuationCheckpoint::capture(
                exchange.full,
                exchange.effort,
                completed,
                delivered,
                endpoint.model.clone(),
            );
        }
        outcome
    }

    /// Service the socket until it sees the idle deadline or unexpected traffic.
    async fn idle(&mut self) {
        if let Some(link) = &mut self.link {
            link.connection.idle().await;
        }
        self.link = None;
    }

    fn capture_token(&mut self, token: Option<&str>) {
        if self.turn.routing_token.is_none() {
            self.turn.routing_token = token.map(str::to_owned);
        }
    }
}

fn recovery_code(code: Option<&str>) -> bool {
    matches!(code, Some("previous_response_not_found" | "websocket_connection_limit_reached"))
}
