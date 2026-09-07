//! Shared WebSocket session for the Responses API providers.

#![doc = include_str!("../../docs/websocket.md")]

use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, Stream, StreamExt};
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, warn};

use super::mappers::{ResponsesRequestPolicy, build_wire_request};
use super::streaming::{ResponsesStreamEvent, process_response_stream};
use crate::provider::stream_from;
use crate::{Context, LlmError, LlmResponseStream, ProviderError, ProviderErrorKind, Result};

/// Called when the WebSocket handshake fails with an authentication error, so
/// the owning provider can drop cached credentials before the next attempt.
pub(crate) type AuthenticationFailureHook = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Everything a provider supplies to route its turns over the shared pool.
pub(crate) struct WsRequestParams {
    pub(crate) ws_url: String,
    pub(crate) handshake_headers: HeaderMap,
    pub(crate) policy: ResponsesRequestPolicy,
    pub(crate) on_authentication_failure: Option<AuthenticationFailureHook>,
}

/// Stream one `context` turn over the pooled WebSocket transport, mapping
/// server events into `LlmResponse`s with the existing SSE pipeline. Request
/// mapping failures surface as the stream's single item, like the HTTP path.
pub(crate) fn stream_via_websocket(params: WsRequestParams, model: String, context: Context) -> LlmResponseStream {
    stream_from(
        async move {
            let wire_body = build_wire_request(&model, &context, &params.policy)?;
            Ok((params, model, context, wire_body))
        },
        |(params, model, context, wire_body)| {
            let events: super::transport::ResponsesEventStream =
                Box::pin(turn_events(params, model, context, wire_body));
            Box::pin(process_response_stream(events))
        },
    )
}

/// The provider chosen by the spike: both backends expose the Responses
/// resource at `wss://…/responses`, reached by upgrading the scheme of the
/// regular HTTP endpoint.
pub(crate) fn derive_ws_url(http_url: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(http_url)
        .map_err(|error| LlmError::ProviderRequest(format!("invalid provider URL {http_url}: {error}")))?;
    let scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        other => {
            return Err(LlmError::ProviderRequest(format!(
                "provider URL scheme {other:?} cannot be upgraded to WebSocket"
            )));
        }
    };
    url.set_scheme(scheme).map_err(|()| LlmError::ProviderRequest(format!("invalid provider URL {http_url}")))?;
    let path = url.path().trim_end_matches('/').to_string();
    if path.ends_with("/responses") {
        url.set_path(&path);
    } else {
        url.set_path(&format!("{path}/responses"));
    }
    Ok(String::from(url))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    provider: String,
    url: String,
    auth_fingerprint: u64,
    model: String,
}

/// Shared across provider instances for the process lifetime; entries are
/// keyed by endpoint + credentials + model, so distinct providers never
/// contend beyond the momentary map lock.
#[derive(Default)]
struct WsSessionPool {
    entries: Mutex<HashMap<PoolKey, PoolEntry>>,
}

#[derive(Default)]
struct PoolEntry {
    connect_lock: Arc<AsyncMutex<()>>,
    session: Option<Arc<WsSession>>,
}

impl WsSessionPool {
    fn global() -> &'static Self {
        static POOL: OnceLock<WsSessionPool> = OnceLock::new();
        POOL.get_or_init(Self::default)
    }

    async fn get_or_connect(&self, key: &PoolKey, params: &WsRequestParams) -> Result<Arc<WsSession>> {
        let connect_lock = {
            let mut entries = self.entries.lock().expect("pool lock");
            entries.entry(key.clone()).or_default().connect_lock.clone()
        };
        let _guard = connect_lock.lock().await;
        if let Some(session) = self.live_session(key) {
            return Ok(session);
        }
        let session = WsSession::connect(params).await?;
        self.entries.lock().expect("pool lock").entry(key.clone()).or_default().session = Some(session.clone());
        Ok(session)
    }

    fn live_session(&self, key: &PoolKey) -> Option<Arc<WsSession>> {
        let entries = self.entries.lock().expect("pool lock");
        entries.get(key).and_then(|entry| entry.session.clone()).filter(|session| !session.is_closed())
    }

    fn remove(&self, key: &PoolKey) {
        self.entries.lock().expect("pool lock").remove(key);
    }
}

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// One live connection: the write half behind an async mutex, a background
/// reader routing frames to lanes, and the incremental-input state per lane.
struct WsSession {
    closed: AtomicBool,
    writer: AsyncMutex<SplitSink<WsStream, Message>>,
    inner: Mutex<SessionInner>,
}

#[derive(Default)]
struct SessionInner {
    lanes: HashMap<Option<String>, mpsc::UnboundedSender<LaneEvent>>,
    lane_states: HashMap<Option<String>, LaneState>,
}

#[derive(Clone)]
struct LaneState {
    response_id: Option<String>,
    input_items: Vec<Value>,
    params_hash: u64,
}

/// What one planned turn will send and remember.
struct TurnPlan {
    lane: Option<String>,
    previous_response_id: Option<String>,
    full_input: Vec<Value>,
    params_hash: u64,
    is_incremental: bool,
}

/// An event routed to a lane's awaiting turn.
enum LaneEvent {
    Responses(Result<ResponsesStreamEvent>),
    WsError { error: WsErrorBody, status: Option<u16> },
    TransportClosed(String),
}

impl WsSession {
    async fn connect(params: &WsRequestParams) -> Result<Arc<Self>> {
        let request = handshake_request(&params.ws_url, &params.handshake_headers)?;
        match connect_async(request).await {
            Ok((stream, _)) => {
                let (writer, reader) = stream.split();
                let session = Arc::new(Self {
                    closed: AtomicBool::new(false),
                    writer: AsyncMutex::new(writer),
                    inner: Mutex::new(SessionInner::default()),
                });
                tokio::spawn(reader_loop(session.clone(), reader));
                Ok(session)
            }
            Err(error) => {
                let error = LlmError::from(error);
                if error.provider().is_some_and(|provider| provider.kind == ProviderErrorKind::Authentication)
                    && let Some(hook) = &params.on_authentication_failure
                {
                    hook().await;
                }
                Err(error)
            }
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    async fn send_text(&self, frame: &Value) -> Result<()> {
        let text = serde_json::to_string(frame)?;
        self.writer.lock().await.send(Message::text(text)).await?;
        Ok(())
    }

    fn subscribe(&self, lane: Option<String>, sender: mpsc::UnboundedSender<LaneEvent>) {
        self.inner.lock().expect("session lock").lanes.insert(lane, sender);
    }

    fn lane_state(&self, lane: Option<&str>) -> Option<LaneState> {
        self.inner.lock().expect("session lock").lane_states.get(&lane.map(String::from)).cloned()
    }

    fn record_success(&self, lane: Option<&str>, response_id: Option<String>, plan: &TurnPlan) {
        let state = LaneState { response_id, input_items: plan.full_input.clone(), params_hash: plan.params_hash };
        self.inner.lock().expect("session lock").lane_states.insert(lane.map(String::from), state);
    }

    fn evict_lane(&self, lane: Option<&str>) {
        self.inner.lock().expect("session lock").lane_states.remove(&lane.map(String::from));
    }

    fn close(&self, reason: &str) {
        self.closed.store(true, Ordering::SeqCst);
        let lanes = std::mem::take(&mut self.inner.lock().expect("session lock").lanes);
        for (_, sender) in lanes {
            sender.send(LaneEvent::TransportClosed(reason.to_string())).ok();
        }
    }

    fn route_frame(&self, text: &str) {
        let frame = match serde_json::from_str::<WsServerFrame>(text) {
            Ok(frame) => frame,
            Err(error) => {
                warn!(%error, frame = %text, "undecodable WebSocket frame from Responses endpoint");
                return;
            }
        };
        match frame {
            WsServerFrame::Message { stream_id, message } => {
                let event = serde_json::from_value::<ResponsesStreamEvent>(message).map_err(|error| {
                    ProviderError::stream_interrupted(format!("Invalid Responses event over WebSocket: {error}")).into()
                });
                self.deliver(stream_id.as_deref(), LaneEvent::Responses(event));
            }
            WsServerFrame::Error { status, stream_id, error } => {
                self.deliver(stream_id.as_deref(), LaneEvent::WsError { error, status });
            }
        }
    }

    fn deliver(&self, stream_id: Option<&str>, event: LaneEvent) {
        let lanes = self.inner.lock().expect("session lock");
        let sender = lanes.lanes.get(&stream_id.map(String::from)).cloned();
        drop(lanes);
        if let Some(sender) = sender {
            sender.send(event).ok();
        } else {
            warn!(?stream_id, "discarding WebSocket frame for a lane with no awaiting turn");
        }
    }
}

async fn reader_loop(session: Arc<WsSession>, mut reader: SplitStream<WsStream>) {
    while let Some(item) = reader.next().await {
        match item {
            Ok(Message::Text(text)) => session.route_frame(&text),
            Ok(Message::Close(_)) => {
                session.close("server closed the WebSocket connection");
                return;
            }
            Ok(_) => {}
            Err(error) => {
                session.close(&error.to_string());
                return;
            }
        }
    }
    session.close("WebSocket connection ended without a close frame");
}

fn handshake_request(ws_url: &str, headers: &HeaderMap) -> Result<Request<()>> {
    let mut request = ws_url.into_client_request()?;
    for (name, value) in headers {
        request.headers_mut().insert(name.clone(), value.clone());
    }
    Ok(request)
}

/// The lane a turn belongs to, derived from the context's session affinity
/// key. Keys outside the protocol's `stream_id` alphabet fall back to the
/// default lane rather than sending something the server would reject.
fn lane_key(context: &Context) -> Option<String> {
    let key = context.session_affinity_key()?;
    let valid = !key.is_empty()
        && key.len() <= 256
        && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid {
        Some(key.to_string())
    } else {
        warn!(key, "session_affinity_key is not a valid stream_id; falling back to the default lane");
        None
    }
}

/// Build the `response.create` envelope for one turn: the full wire request
/// minus transport-only fields, plus `stream_id`/`previous_response_id`, with
/// the input window replaced by just the new items when the lane can continue.
fn build_create_envelope(wire_body: &Value, lane: Option<&str>, lane_state: Option<&LaneState>) -> (Value, TurnPlan) {
    let input = wire_body.get("input").and_then(Value::as_array).cloned().unwrap_or_default();
    let params_hash = params_signature(wire_body);

    let continuable = lane_state.is_some_and(|state| {
        state.response_id.is_some()
            && state.params_hash == params_hash
            && input.starts_with(&state.input_items)
            && input.len() > state.input_items.len()
    });

    let (previous_response_id, sent_input, is_incremental) = match continuable.then_some(lane_state).flatten() {
        Some(state) => {
            let suffix = input[state.input_items.len()..].to_vec();
            (state.response_id.clone(), suffix, true)
        }
        None => (None, input.clone(), false),
    };

    let mut envelope = Map::new();
    envelope.insert("type".to_string(), Value::from("response.create"));
    if let Some(lane) = lane {
        envelope.insert("stream_id".to_string(), Value::from(lane));
    }
    if let Some(previous) = &previous_response_id {
        envelope.insert("previous_response_id".to_string(), Value::from(previous.as_str()));
    }
    if let Some(body) = wire_body.as_object() {
        for (key, value) in body {
            if key == "stream" || key == "background" || key == "previous_response_id" {
                continue;
            }
            envelope.insert(key.clone(), value.clone());
        }
    }
    envelope.insert("input".to_string(), Value::Array(sent_input));

    let plan =
        TurnPlan { lane: lane.map(String::from), previous_response_id, full_input: input, params_hash, is_incremental };
    (Value::Object(envelope), plan)
}

/// Hash of everything in the wire request except `input`: when it changes
/// (model, instructions, tools, settings), lanes must resend the full window.
fn params_signature(wire_body: &Value) -> u64 {
    let mut signature = wire_body.as_object().cloned().unwrap_or_default();
    signature.remove("input");
    hash_json(&Value::Object(signature))
}

fn hash_json(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    serde_json::to_string(value).unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

/// Header fingerprint grouping pooled connections by effective credentials.
fn auth_fingerprint(headers: &HeaderMap) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut header_list: Vec<(String, Vec<u8>)> =
        headers.iter().map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec())).collect();
    header_list.sort();
    for (name, value) in header_list {
        name.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    hasher.finish()
}

fn pool_key(params: &WsRequestParams, model: &str) -> PoolKey {
    PoolKey {
        provider: params.policy.provider.parser_name().to_string(),
        url: params.ws_url.clone(),
        auth_fingerprint: auth_fingerprint(&params.handshake_headers),
        model: model.to_string(),
    }
}

/// Drive one turn against the pool: connect (or reuse), send the envelope,
/// route lane events, and apply the retry ladder (full resend on
/// `previous_response_not_found`, reconnect on transport failure).
fn turn_events(
    params: WsRequestParams,
    model: String,
    context: Context,
    wire_body: Value,
) -> impl Stream<Item = Result<ResponsesStreamEvent>> + Send {
    async_stream::stream! {
        let lane = lane_key(&context);
        let key = pool_key(&params, &model);
        let pool = WsSessionPool::global();

        let mut retried_not_found = false;
        let mut reconnected = false;

        'attempt: loop {
            let session = match pool.get_or_connect(&key, &params).await {
                Ok(session) => session,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };

            let (envelope, plan) = {
                let state = session.lane_state(lane.as_deref());
                build_create_envelope(&wire_body, lane.as_deref(), state.as_ref())
            };
            debug!(model = %model, lane = ?plan.lane, incremental = plan.is_incremental,
                previous_response_id = ?plan.previous_response_id, "sending response.create over WebSocket");

            let (sender, mut receiver) = mpsc::unbounded_channel();
            session.subscribe(lane.clone(), sender);
            if let Err(error) = session.send_text(&envelope).await {
                if reconnected {
                    yield Err(error);
                    return;
                }
                reconnected = true;
                session.evict_lane(lane.as_deref());
                pool.remove(&key);
                continue 'attempt;
            }

            let mut response_id: Option<String> = None;
            while let Some(event) = receiver.recv().await {
                match event {
                    LaneEvent::Responses(Ok(event)) => {
                        if let ResponsesStreamEvent::Created(created) = &event {
                            response_id = Some(created.response.id.clone());
                        }
                        let terminal = matches!(event, ResponsesStreamEvent::Completed(_) | ResponsesStreamEvent::Incomplete(_));
                        if terminal {
                            session.record_success(lane.as_deref(), response_id.clone(), &plan);
                        }
                        yield Ok(event);
                        if terminal {
                            return;
                        }
                    }
                    LaneEvent::Responses(Err(error)) => {
                        session.evict_lane(lane.as_deref());
                        yield Err(error);
                        return;
                    }
                    LaneEvent::WsError { error, status } => {
                        let code = error.code.as_deref();
                        if code == Some("previous_response_not_found") && plan.is_incremental && !retried_not_found {
                            retried_not_found = true;
                            session.evict_lane(lane.as_deref());
                            continue 'attempt;
                        }
                        if code == Some("websocket_connection_limit_reached") && !reconnected {
                            reconnected = true;
                            session.evict_lane(lane.as_deref());
                            pool.remove(&key);
                            continue 'attempt;
                        }
                        session.evict_lane(lane.as_deref());
                        yield Err(map_ws_error(&error, status).into());
                        return;
                    }
                    LaneEvent::TransportClosed(reason) => {
                        if !reconnected {
                            reconnected = true;
                            session.evict_lane(lane.as_deref());
                            pool.remove(&key);
                            continue 'attempt;
                        }
                        yield Err(ProviderError::stream_interrupted(reason).into());
                        return;
                    }
                }
            }

            session.evict_lane(lane.as_deref());
            yield Err(ProviderError::stream_interrupted(
                "WebSocket lane closed before the turn completed".to_string(),
            ).into());
            return;
        }
    }
}

/// Map a server `error` frame onto the shared `ProviderError` taxonomy so the
/// retry behavior of HTTP responses is inherited.
fn map_ws_error(error: &WsErrorBody, status: Option<u16>) -> ProviderError {
    let code = error.code.clone();
    let message = error.message.clone().unwrap_or_else(|| "Responses WebSocket request failed".to_string());
    let kind = match code.as_deref() {
        Some("rate_limit_exceeded") => ProviderErrorKind::RateLimit,
        Some("server_error" | "websocket_connection_limit_reached") => ProviderErrorKind::Server,
        Some("invalid_stream_id" | "websocket_stream_limit_reached") => ProviderErrorKind::Api,
        _ => match status {
            Some(status) => ProviderError::from_http_status(status, message.clone()).kind,
            None => ProviderErrorKind::Unknown,
        },
    };
    ProviderError::new(kind, message).with_code(code).with_http_metadata(status, None)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WsServerFrame {
    Message {
        #[serde(default)]
        stream_id: Option<String>,
        message: Value,
    },
    Error {
        #[serde(default)]
        status: Option<u16>,
        #[serde(default)]
        stream_id: Option<String>,
        error: WsErrorBody,
    },
}

#[derive(Debug, Deserialize)]
struct WsErrorBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl From<tokio_tungstenite::tungstenite::Error> for LlmError {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        use tokio_tungstenite::tungstenite::Error as WsError;
        match error {
            WsError::Http(response) => {
                let status = response.status().as_u16();
                ProviderError::from_http_status(status, format!("WebSocket handshake failed with status {status}"))
                    .into()
            }
            WsError::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
                ProviderError::timeout(io.to_string()).into()
            }
            WsError::Io(io) => ProviderError::network(io.to_string()).into(),
            WsError::Tls(tls) => ProviderError::network(format!("TLS error: {tls}")).into(),
            WsError::Url(url) => LlmError::ProviderRequest(format!("invalid WebSocket URL: {url}")),
            other => ProviderError::stream_interrupted(other.to_string()).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ProviderErrorKind};
    use serde_json::json;

    fn wire_body(model: &str, messages: &[ChatMessage]) -> Value {
        build_wire_request(model, &Context::new(messages.to_vec(), vec![]), &ResponsesRequestPolicy::openai()).unwrap()
    }

    #[test]
    fn derive_ws_url_upgrades_openai_default() {
        assert_eq!(derive_ws_url("https://api.openai.com/v1").unwrap(), "wss://api.openai.com/v1/responses");
    }

    #[test]
    fn derive_ws_url_upgrades_local_gateways() {
        assert_eq!(derive_ws_url("http://127.0.0.1:8080").unwrap(), "ws://127.0.0.1:8080/responses");
        assert_eq!(derive_ws_url("https://gateway.internal/v1/").unwrap(), "wss://gateway.internal/v1/responses");
    }

    #[test]
    fn derive_ws_url_keeps_existing_responses_path() {
        assert_eq!(
            derive_ws_url("https://chatgpt.com/backend-api/codex/responses").unwrap(),
            "wss://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            derive_ws_url("https://chatgpt.com/backend-api/codex/responses/").unwrap(),
            "wss://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn derive_ws_url_preserves_query_parameters() {
        assert_eq!(
            derive_ws_url("https://api.openai.com/v1?api-key=secret").unwrap(),
            "wss://api.openai.com/v1/responses?api-key=secret"
        );
    }

    #[test]
    fn derive_ws_url_rejects_invalid_urls() {
        assert!(matches!(derive_ws_url("not a url"), Err(LlmError::ProviderRequest(_))));
        assert!(matches!(derive_ws_url("ftp://api.openai.com/v1"), Err(LlmError::ProviderRequest(_))));
    }

    #[test]
    fn lane_key_passes_valid_affinity_keys_through() {
        let mut context = Context::new(vec![ChatMessage::user("hi")], vec![]);
        context.set_session_affinity_key(Some("conversation-123_v4.blue".to_string()));
        assert_eq!(lane_key(&context).as_deref(), Some("conversation-123_v4.blue"));
    }

    #[test]
    fn lane_key_without_affinity_uses_the_default_lane() {
        assert_eq!(lane_key(&Context::new(vec![ChatMessage::user("hi")], vec![])), None);
    }

    #[test]
    fn lane_key_falls_back_to_default_lane_for_invalid_keys() {
        for key in ["", "has spaces", "slash/ed", &"x".repeat(257)] {
            let mut context = Context::new(vec![ChatMessage::user("hi")], vec![]);
            context.set_session_affinity_key(Some(key.to_string()));
            assert_eq!(lane_key(&context), None, "{key:?} must fall back to the default lane");
        }
    }

    #[test]
    fn first_turn_sends_a_full_envelope() {
        let body = wire_body("gpt-5.6", &[ChatMessage::user("Hello")]);
        let (envelope, plan) = build_create_envelope(&body, Some("lane-1"), None);

        assert_eq!(envelope["type"], "response.create");
        assert_eq!(envelope["stream_id"], "lane-1");
        assert!(envelope.get("previous_response_id").is_none());
        assert!(envelope.get("stream").is_none());
        assert!(envelope.get("background").is_none());
        assert_eq!(envelope["store"], false);
        assert_eq!(envelope["model"], "gpt-5.6");
        assert_eq!(envelope["input"].as_array().unwrap().len(), 1);
        assert_eq!(plan.full_input, body["input"].as_array().unwrap().as_slice());
        assert!(!plan.is_incremental);
    }

    #[test]
    fn append_only_turns_send_only_the_suffix() {
        let first = wire_body("gpt-5.6", &[ChatMessage::user("Hello")]);
        let (_, plan) = build_create_envelope(&first, Some("lane-1"), None);
        let state = LaneState {
            response_id: Some("resp_1".to_string()),
            input_items: plan.full_input.clone(),
            params_hash: plan.params_hash,
        };

        let second = wire_body("gpt-5.6", &[ChatMessage::user("Hello"), ChatMessage::user("More")]);
        let (envelope, plan2) = build_create_envelope(&second, Some("lane-1"), Some(&state));

        assert_eq!(envelope["previous_response_id"], "resp_1");
        assert_eq!(envelope["input"].as_array().unwrap().len(), 1);
        assert_eq!(envelope["input"][0], second["input"][1]);
        assert!(plan2.is_incremental);
        assert_eq!(plan2.full_input, second["input"].as_array().unwrap().as_slice());
    }

    #[test]
    fn compaction_restarts_the_chain() {
        let long: Vec<ChatMessage> = (0..3).map(|i| ChatMessage::user(format!("msg {i}"))).collect();
        let before = wire_body("gpt-5.6", &long);
        let (_, plan) = build_create_envelope(&before, Some("lane-1"), None);
        let state = LaneState {
            response_id: Some("resp_1".to_string()),
            input_items: plan.full_input.clone(),
            params_hash: plan.params_hash,
        };

        let compacted = wire_body("gpt-5.6", &[ChatMessage::user("summary of everything")]);
        let (envelope, plan) = build_create_envelope(&compacted, Some("lane-1"), Some(&state));

        assert!(envelope.get("previous_response_id").is_none());
        assert!(!plan.is_incremental);
        assert_eq!(envelope["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn model_change_forces_a_full_resend() {
        let first = wire_body("gpt-5.6", &[ChatMessage::user("Hello")]);
        let (_, plan) = build_create_envelope(&first, Some("lane-1"), None);
        let state = LaneState {
            response_id: Some("resp_1".to_string()),
            input_items: plan.full_input.clone(),
            params_hash: plan.params_hash,
        };

        let changed = wire_body("gpt-5.5", &[ChatMessage::user("Hello"), ChatMessage::user("More")]);
        let (envelope, plan) = build_create_envelope(&changed, Some("lane-1"), Some(&state));

        assert!(envelope.get("previous_response_id").is_none());
        assert!(!plan.is_incremental);
    }

    #[test]
    fn identical_input_resends_full_rather_than_an_empty_suffix() {
        let body = wire_body("gpt-5.6", &[ChatMessage::user("Hello")]);
        let (_, plan) = build_create_envelope(&body, Some("lane-1"), None);
        let state = LaneState {
            response_id: Some("resp_1".to_string()),
            input_items: plan.full_input.clone(),
            params_hash: plan.params_hash,
        };

        let (envelope, plan) = build_create_envelope(&body, Some("lane-1"), Some(&state));

        assert!(envelope.get("previous_response_id").is_none());
        assert!(!plan.is_incremental);
        assert_eq!(envelope["input"], body["input"]);
    }

    #[test]
    fn default_lane_omits_stream_id() {
        let body = wire_body("gpt-5.6", &[ChatMessage::user("Hello")]);
        let (envelope, _) = build_create_envelope(&body, None, None);
        assert!(envelope.get("stream_id").is_none());
    }

    #[test]
    fn message_frames_unwrap_into_responses_events() {
        let frame = json!({
            "type": "message",
            "stream_id": "lane-1",
            "message": {"type": "response.created", "response": {"id": "resp_1"}}
        });
        let parsed = serde_json::from_value::<WsServerFrame>(frame).unwrap();
        match parsed {
            WsServerFrame::Message { stream_id, message } => {
                assert_eq!(stream_id.as_deref(), Some("lane-1"));
                let event = serde_json::from_value::<ResponsesStreamEvent>(message).unwrap();
                assert!(matches!(event, ResponsesStreamEvent::Created(_)));
            }
            other @ WsServerFrame::Error { .. } => panic!("expected a message frame, got {other:?}"),
        }
    }

    #[test]
    fn message_frames_without_stream_id_target_the_default_lane() {
        let frame = json!({
            "type": "message",
            "message": {"type": "response.output_text.delta", "delta": "hi"}
        });
        let parsed = serde_json::from_value::<WsServerFrame>(frame).unwrap();
        match parsed {
            WsServerFrame::Message { stream_id, .. } => assert_eq!(stream_id, None),
            other @ WsServerFrame::Error { .. } => panic!("expected a message frame, got {other:?}"),
        }
    }

    #[test]
    fn error_frames_decode_the_documented_shape() {
        let frame = json!({
            "type": "error",
            "status": 400,
            "stream_id": "lane-1",
            "error": {"type": "invalid_request_error", "code": "previous_response_not_found", "message": "No cached response", "param": null}
        });
        match serde_json::from_value::<WsServerFrame>(frame).unwrap() {
            WsServerFrame::Error { status, stream_id, error } => {
                assert_eq!(status, Some(400));
                assert_eq!(stream_id.as_deref(), Some("lane-1"));
                assert_eq!(error.code.as_deref(), Some("previous_response_not_found"));
                assert_eq!(error.message.as_deref(), Some("No cached response"));
            }
            other @ WsServerFrame::Message { .. } => panic!("expected an error frame, got {other:?}"),
        }
    }
    #[test]
    fn error_frame_codes_map_onto_the_provider_taxonomy() {
        let map = |code: &str, status: Option<u16>| {
            let body = WsErrorBody { code: Some(code.to_string()), message: Some("boom".into()) };
            let error = map_ws_error(&body, status);
            (error.kind, error.is_retryable())
        };

        assert_eq!(map("rate_limit_exceeded", None).0, ProviderErrorKind::RateLimit);
        assert_eq!(map("server_error", Some(500)).0, ProviderErrorKind::Server);
        assert_eq!(map("websocket_connection_limit_reached", None).0, ProviderErrorKind::Server);
        assert_eq!(map("invalid_stream_id", Some(400)).0, ProviderErrorKind::Api);
        assert_eq!(map("websocket_stream_limit_reached", Some(400)).0, ProviderErrorKind::Api);
        assert_eq!(map("mystery_code", Some(400)).0, ProviderErrorKind::Api);
        assert_eq!(map("mystery_code", None).0, ProviderErrorKind::Unknown);

        let (kind, retryable) = map("rate_limit_exceeded", None);
        assert_eq!(kind, ProviderErrorKind::RateLimit);
        assert!(retryable);
        assert!(!map("invalid_stream_id", Some(400)).1);
    }

    #[test]
    fn five_hundred_status_without_a_code_is_a_server_error() {
        let body = WsErrorBody { code: None, message: None };
        let error = map_ws_error(&body, Some(503));
        assert_eq!(error.kind, ProviderErrorKind::Server);
        assert_eq!(error.http_status, Some(503));
    }

    #[test]
    fn tungstenite_errors_map_by_kind() {
        use tokio_tungstenite::tungstenite::Error as WsError;
        use tokio_tungstenite::tungstenite::error::UrlError;

        let timed_out = WsError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "slow"));
        assert!(matches!(LlmError::from(timed_out).provider().unwrap().kind, ProviderErrorKind::Timeout));

        let refused = WsError::Io(std::io::Error::other("refused"));
        assert!(matches!(LlmError::from(refused).provider().unwrap().kind, ProviderErrorKind::Network));

        let relative = WsError::Url(UrlError::NoHostName);
        assert!(matches!(LlmError::from(relative), LlmError::ProviderRequest(_)));

        let protocol = WsError::Utf8("not utf-8".to_string());
        assert!(matches!(LlmError::from(protocol).provider().unwrap().kind, ProviderErrorKind::StreamInterrupted));
    }

    #[test]
    fn auth_fingerprint_is_order_independent() {
        let mut a = HeaderMap::new();
        a.insert("authorization", "secret".parse().unwrap());
        a.insert("chatgpt-account-id", "account".parse().unwrap());
        let mut b = HeaderMap::new();
        b.insert("chatgpt-account-id", "account".parse().unwrap());
        b.insert("authorization", "secret".parse().unwrap());
        assert_eq!(auth_fingerprint(&a), auth_fingerprint(&b));

        let mut other = a.clone();
        other.insert("authorization", "different".parse().unwrap());
        assert_ne!(auth_fingerprint(&a), auth_fingerprint(&other));
    }
}
