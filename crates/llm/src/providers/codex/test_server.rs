use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Uri};
use axum::routing::get;
use futures::SinkExt;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) struct FakeResponsesWebsocketServer {
    pub base_url: String,
    requests: mpsc::UnboundedReceiver<CapturedRequest>,
    state: Arc<ServerState>,
    closed: mpsc::UnboundedReceiver<usize>,
    pongs: mpsc::UnboundedReceiver<()>,
    task: JoinHandle<()>,
}

pub(super) struct CapturedRequest {
    pub connection: usize,
    pub headers: HeaderMap,
    pub uri: Uri,
    pub body: Value,
    pub effective_input: Vec<Value>,
}

pub(super) struct Reply {
    pub frames: Vec<Message>,
    pub release: Option<oneshot::Receiver<()>>,
}

impl Reply {
    pub fn events(events: Vec<Value>) -> Self {
        Self {
            frames: events.into_iter().map(|event| Message::Text(event.to_string().into())).collect(),
            release: None,
        }
    }
}

impl FakeResponsesWebsocketServer {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let (sender, requests) = mpsc::unbounded_channel();
        let (close_sender, closed) = mpsc::unbounded_channel();
        let (pong_sender, pongs) = mpsc::unbounded_channel();
        let state = Arc::new(ServerState {
            closed: close_sender,
            pongs: pong_sender,
            handshake_token: Mutex::new(None),
            requests: sender,
            replies: Mutex::new(replies.into()),
            connections: Mutex::new(0),
            shutdown: CancellationToken::new(),
        });
        let router = axum::Router::new()
            .route("/responses", get(upgrade))
            .route("/base/responses", get(upgrade))
            .route("/reject/{status}/responses", get(reject))
            .route("/reject-body/{mode}/responses", get(reject_body))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { base_url, requests, state, closed, pongs, task }
    }

    pub fn handshake_token(&self, token: &str) {
        *self.state.handshake_token.lock().unwrap() = Some(token.into());
    }

    pub async fn closed(&mut self) -> usize {
        self.closed.recv().await.unwrap()
    }

    pub async fn pong(&mut self) {
        self.pongs.recv().await.unwrap();
    }

    pub async fn captured(&mut self) -> CapturedRequest {
        self.requests.recv().await.expect("fake server stopped before receiving request")
    }
}

impl Drop for FakeResponsesWebsocketServer {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
        self.task.abort();
    }
}

pub(super) fn text_events(id: &str, text: &str) -> Vec<Value> {
    vec![
        json!({"type":"response.created", "response":{"id":id}}),
        json!({"type":"response.output_text.delta", "delta":text}),
        json!({"type":"response.completed", "response":{"id":id,"status":"completed", "output":[
            {"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}
        ]}}),
    ]
}

struct ServerState {
    requests: mpsc::UnboundedSender<CapturedRequest>,
    replies: Mutex<VecDeque<Reply>>,
    connections: Mutex<usize>,
    shutdown: CancellationToken,
    closed: mpsc::UnboundedSender<usize>,
    pongs: mpsc::UnboundedSender<()>,
    handshake_token: Mutex<Option<String>>,
}

async fn upgrade(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    uri: Uri,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let connection = {
        let mut count = state.connections.lock().unwrap();
        *count += 1;
        *count
    };
    let token = state.handshake_token.lock().unwrap().clone();
    let mut response = ws.on_upgrade(move |socket| async move {
        tokio::select! {
            () = state.shutdown.cancelled() => {},
            () = serve(socket, &state, connection, headers, uri) => {},
        }
        let _ = state.closed.send(connection);
    });
    response.headers_mut().insert("x-request-id", "upgrade-request".parse().unwrap());
    if let Some(token) = token {
        response.headers_mut().insert("x-codex-turn-state", token.parse().unwrap());
    }
    response
}

async fn reject(axum::extract::Path(status): axum::extract::Path<u16>) -> impl axum::response::IntoResponse {
    (
        axum::http::StatusCode::from_u16(status).unwrap(),
        [("x-request-id", "upgrade-rejected")],
        axum::Json(json!({"error":{"code":"rejected","message":"Upgrade rejected"}})),
    )
}

async fn reject_body(axum::extract::Path(mode): axum::extract::Path<String>) -> impl axum::response::IntoResponse {
    let body = if mode == "large" {
        json!({"error":{"code":"oversized","message":"sensitive".repeat(10000)}}).to_string()
    } else {
        "sensitive proxy response".into()
    };
    (axum::http::StatusCode::TOO_MANY_REQUESTS, [("x-request-id", "upgrade-rejected")], body)
}

async fn serve(mut socket: WebSocket, state: &ServerState, connection: usize, headers: HeaderMap, uri: Uri) {
    let mut lineage: HashMap<String, Vec<Value>> = HashMap::new();
    let mut sequence = 0;
    while let Some(Ok(message)) = socket.recv().await {
        if matches!(message, Message::Pong(_)) {
            let _ = state.pongs.send(());
        }
        let Message::Text(text) = message else {
            continue;
        };
        let body: Value = serde_json::from_str(&text).unwrap();
        let mut effective = if let Some(id) = body["previous_response_id"].as_str() {
            let Some(prefix) = lineage.get(id) else {
                let error =
                    json!({"type":"error","error":{"code":"previous_response_not_found","message":"Unknown response"}});
                if socket.send(Message::Text(error.to_string().into())).await.is_err() {
                    return;
                }
                continue;
            };
            prefix.clone()
        } else {
            Vec::new()
        };
        effective.extend(body["input"].as_array().unwrap().clone());
        let invalid_call = effective.iter().any(|item| {
            item["type"] == "function_call_output"
                && !effective.iter().any(|call| call["type"] == "function_call" && call["call_id"] == item["call_id"])
        });
        if invalid_call {
            let error =
                json!({"type":"error","status":400,"error":{"code":"invalid_call_id","message":"Unknown tool call"}});
            let _ = socket.send(Message::Text(error.to_string().into())).await;
            continue;
        }
        let _ = state.requests.send(CapturedRequest {
            connection,
            headers: headers.clone(),
            uri: uri.clone(),
            body,
            effective_input: effective.clone(),
        });
        sequence += 1;
        let reply = state
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Reply::events(text_events(&format!("resp_{connection}_{sequence}"), "Hello")));
        if let Some(release) = reply.release {
            let _ = release.await;
        }
        for frame in reply.frames {
            if let Message::Text(text) = &frame {
                let event: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                if event["type"] == "response.completed"
                    && let (Some(id), Some(output)) =
                        (event["response"]["id"].as_str(), event["response"]["output"].as_array())
                {
                    let mut prefix = effective.clone();
                    for item in output {
                        match item["type"].as_str() {
                                Some("message") => {
                                    let text: String = item["content"].as_array().unwrap().iter().filter_map(|part| part["text"].as_str()).collect();
                                    prefix.push(json!({"type":"message","role":"assistant","content":text}));
                                }
                                Some("function_call") => prefix.push(json!({"type":"function_call","name":item["name"],"call_id":item["call_id"],"arguments":item["arguments"]})),
                                Some("reasoning") => prefix.push(json!({"type":"reasoning","id":item["id"],"encrypted_content":item["encrypted_content"],"summary":[]})),
                                _ => {},
                            }
                    }
                    lineage.insert(id.into(), prefix);
                }
            }
            if socket.send(frame).await.is_err() {
                return;
            }
        }
        if socket.flush().await.is_err() {
            return;
        }
    }
}
