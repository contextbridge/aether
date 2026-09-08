use async_openai::types::responses::{CreateResponse, Status};
use futures::{SinkExt, StreamExt};
use reqwest::{Client, Url, header::HeaderMap};
use reqwest_websocket::{Message, Upgrade, WebSocket};
use serde_json::{Value, json};
use tokio::time::{Duration, Instant, timeout, timeout_at};

use crate::providers::openai_responses::streaming::{ResponsesErrorEvent, ResponsesStreamEvent};
use crate::{LlmError, ProviderError, ReasoningEffort, Result};

/// How long a socket may sit idle between responses before it is closed.
pub(super) const IDLE_LIFETIME: Duration = Duration::from_mins(5);
/// Sockets older than this are not reused for new requests.
pub(super) const MAX_CONNECTION_AGE: Duration = Duration::from_mins(55);
pub(super) const OPEN_SEND_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for the next model event while a response is streaming.
pub(super) const EVENT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

pub(super) struct Connection {
    socket: WebSocket,
    request_id: Option<String>,
    opened: Instant,
    phase: Phase,
}

impl Connection {
    /// Open a socket, returning it with any routing token issued during the upgrade.
    pub async fn open(client: &Client, url: Url, headers: HeaderMap) -> Result<(Self, Option<String>)> {
        let deadline = Instant::now() + OPEN_SEND_TIMEOUT;
        let upgrade = timeout_at(deadline, client.get(url).headers(headers).upgrade().send())
            .await
            .map_err(|_| ProviderError::timeout("Codex WebSocket upgrade deadline exceeded"))?
            .map_err(open_error)?;
        let request_id = header_value(upgrade.headers(), "x-request-id");
        if upgrade.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            let status = upgrade.status().as_u16();
            let body =
                timeout_at(deadline.min(Instant::now() + REJECTION_BODY_TIMEOUT), rejection_body(upgrade.into_inner()))
                    .await
                    .unwrap_or(Value::Null);
            let mut rejection = serde_json::from_value::<ResponsesErrorEvent>(body).unwrap_or_default();
            rejection.status = Some(status);
            let mut error = ProviderError::from(rejection);
            error.request_id = request_id;
            return Err(error.into());
        }
        let turn_state = header_value(upgrade.headers(), "x-codex-turn-state");
        let socket = timeout_at(deadline, upgrade.into_websocket())
            .await
            .map_err(|_| ProviderError::timeout("Codex WebSocket upgrade deadline exceeded"))?
            .map_err(open_error)?;
        let opened = Instant::now();
        Ok((Self { socket, request_id, opened, phase: Phase::Idle(opened + IDLE_LIFETIME) }, turn_state))
    }

    /// The request ID the server assigned to the upgrade, for error diagnostics.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub fn reusable(&self) -> bool {
        matches!(self.phase, Phase::Idle(until) if Instant::now() < until) && self.opened.elapsed() < MAX_CONNECTION_AGE
    }

    pub async fn send(
        &mut self,
        request: &CreateResponse,
        effort: Option<ReasoningEffort>,
        turn_id: &str,
        routing_token: Option<&str>,
    ) -> Result<()> {
        let mut body = serde_json::to_value(request)?;
        // The SDK cannot represent Codex's `max` reasoning effort.
        if let Some(effort) = effort {
            body["reasoning"]["effort"] = effort.as_str().into();
        }
        body["type"] = "response.create".into();
        body["client_metadata"] = json!({"x-codex-turn-id": turn_id});
        if let Some(token) = routing_token {
            body["client_metadata"]["x-codex-turn-state"] = token.into();
        }
        timeout(OPEN_SEND_TIMEOUT, self.socket.send(Message::Text(body.to_string())))
            .await
            .map_err(|_| ProviderError::timeout("Codex WebSocket send deadline exceeded"))?
            .map_err(|_| ProviderError::stream_interrupted("Codex WebSocket send failed"))?;
        self.phase = Phase::Streaming(Instant::now() + EVENT_IDLE_TIMEOUT);
        Ok(())
    }

    pub async fn receive(&mut self) -> Result<ResponsesStreamEvent> {
        loop {
            let event = match timeout_at(self.phase.deadline(), self.socket.next()).await {
                Err(_) => {
                    return Err(ProviderError::timeout("Codex WebSocket response-event idle deadline exceeded").into());
                }
                Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<ResponsesStreamEvent>(&text)
                    .map_err(|_| ProviderError::stream_interrupted("Invalid Codex WebSocket event"))?,
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {
                    timeout(OPEN_SEND_TIMEOUT, self.socket.flush())
                        .await
                        .map_err(|_| ProviderError::timeout("Codex WebSocket ping deadline exceeded"))?
                        .map_err(|_| ProviderError::stream_interrupted("Codex WebSocket ping failed"))?;
                    continue;
                }
                Ok(Some(Ok(Message::Binary(_)))) => {
                    return Err(ProviderError::stream_interrupted("Unexpected binary Codex WebSocket frame").into());
                }
                Ok(None | Some(Ok(Message::Close { .. }) | Err(_))) => {
                    return Err(
                        ProviderError::stream_interrupted("Codex WebSocket closed before terminal response").into()
                    );
                }
            };
            match &event {
                ResponsesStreamEvent::Metadata(_) => {}
                ResponsesStreamEvent::Completed(completed)
                    if matches!(completed.response.status, Some(Status::Completed)) =>
                {
                    self.phase = Phase::Idle(Instant::now() + IDLE_LIFETIME);
                }
                _ => self.phase = Phase::Streaming(Instant::now() + EVENT_IDLE_TIMEOUT),
            }
            return Ok(event);
        }
    }

    /// Service control frames while idle, returning on unexpected traffic or the idle deadline.
    pub async fn idle(&mut self) {
        while matches!(self.receive().await, Ok(ResponsesStreamEvent::Metadata(_))) {}
    }
}

pub(super) fn responses_url(base: &str) -> Result<Url> {
    let mut url = Url::parse(base).map_err(|_| LlmError::ProviderRequest("Invalid Codex endpoint URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(LlmError::ProviderRequest(
            "Codex endpoint requires HTTP(S), a host, and no userinfo or fragment".into(),
        ));
    }
    url.path_segments_mut()
        .map_err(|()| LlmError::ProviderRequest("Invalid Codex endpoint path".into()))?
        .pop_if_empty()
        .push("responses");
    Ok(url)
}

const REJECTION_BODY_LIMIT: usize = 16 * 1024;
const REJECTION_BODY_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether a response is in flight on the socket, and when waiting for it gives up.
enum Phase {
    /// No response in flight; the socket may be reused until the deadline.
    Idle(Instant),
    /// A response is streaming; the next event must arrive before the deadline.
    Streaming(Instant),
}

impl Phase {
    fn deadline(&self) -> Instant {
        match self {
            Self::Idle(deadline) | Self::Streaming(deadline) => *deadline,
        }
    }
}

async fn rejection_body(mut response: reqwest::Response) -> Value {
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if body.len() + chunk.len() > REJECTION_BODY_LIMIT {
            return Value::Null;
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).unwrap_or(Value::Null)
}

fn open_error(error: reqwest_websocket::Error) -> LlmError {
    match error {
        reqwest_websocket::Error::Handshake(_) => ProviderError::api("Invalid Codex WebSocket upgrade handshake"),
        reqwest_websocket::Error::Reqwest(error) if error.is_timeout() => {
            ProviderError::timeout("Codex WebSocket opening timed out")
        }
        _ => ProviderError::network("Unable to open Codex WebSocket connection"),
    }
    .into()
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
}
