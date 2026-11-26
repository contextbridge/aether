use async_stream::try_stream;
use futures::{SinkExt, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as SocketError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::debug;

use super::client::ResponsesCall;
use super::mappers::replay_output;
use super::streaming::{
    LOST_CONTINUATION_CODES, OutputItem, ResponseEvents, ResponsesCompleted, ResponsesStreamEvent as Event, Status,
};
use super::types::{InputItem, RequestSettings, ResponsesRequest};
use crate::providers::http::{extract_request_id, responses_code};
use crate::{LlmError, ProviderError, Result};

#[derive(Default)]
pub(crate) struct SocketSession {
    idle: Mutex<Option<IdleSocket>>,
    upgrade_refused: Arc<AtomicBool>,
}

pub(crate) enum SocketOutcome {
    Stream(ResponseEvents),
    UpgradeRefused(Box<ResponsesCall>),
}

impl SocketSession {
    pub(crate) fn for_another_owner(&self) -> Self {
        Self { idle: Mutex::default(), upgrade_refused: Arc::clone(&self.upgrade_refused) }
    }

    pub(crate) fn accepts_upgrades(&self) -> bool {
        !self.upgrade_refused.load(Ordering::Relaxed)
    }

    pub(crate) async fn open(self: &Arc<Self>, url: &str, call: ResponsesCall) -> Result<SocketOutcome> {
        let mut turn = match self.take_continued_by(&call) {
            Some(idle) => Turn { socket: idle.socket, call, previous: Some(idle.chain) },
            None => match Socket::connect(url, &call.headers).await {
                Ok(socket) => Turn { socket, call, previous: None },
                Err(ConnectError::UpgradeRefused { status }) => {
                    debug!(status, url, "Responses endpoint refused the WebSocket upgrade; using HTTP from now on");
                    self.upgrade_refused.store(true, Ordering::Relaxed);
                    return Ok(SocketOutcome::UpgradeRefused(Box::new(call)));
                }
                Err(ConnectError::Failed(error)) => return Err(error),
            },
        };

        if let Err(error) = turn.send().await {
            turn.recover(url, error).await?;
        }
        Ok(SocketOutcome::Stream(self.events(turn, url.to_string())))
    }

    fn events(self: &Arc<Self>, mut turn: Turn, url: String) -> ResponseEvents {
        let session = Arc::clone(self);
        Box::pin(try_stream! {
            let mut started = false;
            let mut acknowledged = false;
            let mut output = Vec::new();

            loop {
                let event = match turn.socket.next_text().await {
                    Ok(frame) => Event::parse(&frame, &mut started)?,
                    Err(error) if !acknowledged => {
                        turn.recover(&url, error).await?;
                        continue;
                    }
                    Err(error) => Err(error)?,
                };
                if !acknowledged && turn.previous.is_some() && lost_continuation(&event) {
                    turn.restart(&url).await?;
                    continue;
                }
                acknowledged |= !matches!(event, Event::Ignored);

                match &event {
                    Event::OutputItemDone(done) => output.push(done.item.clone()),
                    Event::Completed(done) | Event::Incomplete(done) => {
                        if let Some(idle) = turn.finish(&done.response, &output) {
                            *session.idle.lock().unwrap_or_else(PoisonError::into_inner) = Some(idle);
                        }
                        yield event;
                        return;
                    }
                    _ => {}
                }
                yield event;
            }
        })
    }

    fn take_continued_by(&self, call: &ResponsesCall) -> Option<IdleSocket> {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        idle.take_if(|idle| idle.socket.opened_at.elapsed() >= MAX_SOCKET_AGE);
        idle.take_if(|idle| idle.headers == call.headers && idle.chain.new_input(&call.request).is_some())
    }
}

const MAX_SOCKET_AGE: Duration = Duration::from_mins(55);
const UPGRADE_REFUSED_STATUSES: [u16; 4] = [404, 405, 426, 501];

struct IdleSocket {
    socket: Socket,
    headers: HeaderMap,
    chain: Chain,
}

struct Chain {
    response_id: String,
    baseline: ResponsesRequest,
}

impl Chain {
    fn new_input<'a>(&self, next: &'a ResponsesRequest) -> Option<&'a [InputItem]> {
        if self.baseline.settings != next.settings {
            return None;
        }
        next.input.strip_prefix(self.baseline.input.as_slice()).filter(|new_input| !new_input.is_empty())
    }
}

struct Turn {
    socket: Socket,
    call: ResponsesCall,
    previous: Option<Chain>,
}

impl Turn {
    async fn send(&mut self) -> Result<()> {
        let request = &self.call.request;
        let continuation = self.previous.as_ref().and_then(|chain| Some((chain, chain.new_input(request)?)));
        let frame = ResponseCreate {
            previous_response_id: continuation.map(|(chain, _)| chain.response_id.as_str()),
            input: continuation.map_or(&request.input, |(_, new_input)| new_input),
            settings: &request.settings,
        };
        debug!(
            continued = frame.previous_response_id.is_some(),
            sent_items = frame.input.len(),
            conversation_items = request.input.len(),
            "Sending Responses turn over WebSocket"
        );
        self.socket.send_text(serde_json::to_string(&frame)?).await
    }

    async fn recover(&mut self, url: &str, error: LlmError) -> Result<()> {
        if self.previous.is_none() {
            return Err(error);
        }
        debug!(%error, "Responses socket went away while idle; starting over");
        self.restart(url).await
    }

    async fn restart(&mut self, url: &str) -> Result<()> {
        self.socket = Socket::connect(url, &self.call.headers).await.map_err(|error| match error {
            ConnectError::Failed(error) => error,
            ConnectError::UpgradeRefused { status } => {
                ProviderError::from_http_status(status, "Responses endpoint stopped accepting WebSockets").into()
            }
        })?;
        self.previous = None;
        self.send().await
    }

    fn finish(self, response: &ResponsesCompleted, output: &[OutputItem]) -> Option<IdleSocket> {
        let Self { socket, call: ResponsesCall { mut request, headers }, .. } = self;
        let completed = matches!(response.status, Some(Status::Completed));
        let response_id = response.id.clone().filter(|id| completed && !id.is_empty())?;
        request.input.extend(replay_output(output)?);
        Some(IdleSocket { socket, headers, chain: Chain { response_id, baseline: request } })
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename = "response.create")]
struct ResponseCreate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<&'a str>,
    input: &'a [InputItem],
    #[serde(flatten)]
    settings: &'a RequestSettings,
}

struct Socket {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    opened_at: Instant,
}

enum ConnectError {
    UpgradeRefused { status: u16 },
    Failed(LlmError),
}

impl Socket {
    async fn connect(url: &str, headers: &HeaderMap) -> std::result::Result<Self, ConnectError> {
        let mut handshake = url
            .replacen("http", "ws", 1)
            .into_client_request()
            .map_err(|error| ConnectError::Failed(LlmError::InvalidArgument(format!("{url}: {error}"))))?;
        handshake.headers_mut().extend(headers.clone());
        handshake.headers_mut().insert("OpenAI-Beta", HeaderValue::from_static("responses_websockets=2026-02-06"));

        let started = Instant::now();
        match connect_async(handshake).await {
            Ok((stream, _)) => {
                debug!(elapsed = ?started.elapsed(), "Opened Responses WebSocket");
                Ok(Self { stream, opened_at: Instant::now() })
            }
            Err(SocketError::Http(response)) => {
                let status = response.status().as_u16();
                if UPGRADE_REFUSED_STATUSES.contains(&status) {
                    return Err(ConnectError::UpgradeRefused { status });
                }
                let body = response.body().as_deref().map_or_default(String::from_utf8_lossy);
                let message = format!("request failed with status {status}: {body}");
                let error = ProviderError::from_http_status(status, message)
                    .with_code(responses_code(&body))
                    .with_request_id(extract_request_id(response.headers()));
                Err(ConnectError::Failed(error.into()))
            }
            Err(error) => Err(ConnectError::Failed(ProviderError::network(error.to_string()).into())),
        }
    }

    async fn send_text(&mut self, frame: String) -> Result<()> {
        self.stream.send(Message::text(frame)).await.map_err(|error| interrupted(&error))
    }

    async fn next_text(&mut self) -> Result<String> {
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Text(text))) => return Ok(text.as_str().to_string()),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Binary(_))) => {
                    return Err(ProviderError::stream_interrupted("Responses socket sent a binary frame").into());
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Err(ProviderError::stream_interrupted("Responses socket closed before completion").into());
                }
                Some(Err(error)) => return Err(interrupted(&error)),
            }
        }
    }
}

fn interrupted(error: &SocketError) -> LlmError {
    ProviderError::stream_interrupted(error.to_string()).into()
}

fn lost_continuation(event: &Event) -> bool {
    let Event::Error { error: Some(error), .. } = event else { return false };
    error.code.as_deref().is_some_and(|code| LOST_CONTINUATION_CODES.contains(&code))
}
