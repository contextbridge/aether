use acp_utils::testing::FakeAgent;
use acp_utils::websocket::WebSocketTransport;
use aether_desktop::agents::{AgentCommand, AgentProfile, Agents, project_agents_file};
use aether_desktop::testing::DesktopHarness;
use aether_desktop::threads::{Thread, ThreadStatus};
use aether_sessions::testing::{TestStore, session_meta, user_message};
use agent_client_protocol::schema::v2::NewSessionResponse;
use gpui::{Entity, TestAppContext};
use serde_json::json;
use std::borrow::Cow;
use std::fs;
use std::path::Path;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderMap, StatusCode};
use tokio_tungstenite::{accept_async, accept_hdr_async};

/// An agent that opens `session_id` and answers every prompt with `reply`.
pub(crate) fn replying(session_id: &str, reply: &str) -> FakeAgent {
    FakeAgent::default().new_session_response(NewSessionResponse::new(session_id)).prompt_reply(reply)
}

/// The text of every item in the thread's conversation that has text.
pub(crate) fn texts(thread: &Thread) -> Vec<String> {
    thread
        .core()
        .map(|core| core.conversation().items().iter().filter_map(|item| item.text().map(Cow::into_owned)).collect())
        .unwrap_or_default()
}

/// Saves a session that ran in `cwd`, titled by its first `prompt` if it has one.
pub(crate) fn save_session(store: &TestStore, session_id: &str, cwd: &str, created_at: &str, prompt: Option<&str>) {
    let mut meta = session_meta(session_id, created_at);
    meta.cwd = cwd.into();
    store.append_meta(session_id, &meta);
    if let Some(prompt) = prompt {
        store.append(session_id, &user_message(prompt));
    }
}

/// Waits until the app has read at least one saved session.
pub(crate) fn wait_for_history(harness: &DesktopHarness, cx: &mut TestAppContext) {
    let history = harness.history(cx);
    harness.run_until(cx, |cx| !history.read(cx).projects().is_empty());
}

/// Opens the context menu of the row tagged `row` and chooses the entry tagged `entry`.
pub(crate) fn choose(harness: &DesktopHarness, cx: &mut TestAppContext, row: &'static str, entry: &'static str) {
    harness.right_click(cx, row).unwrap();
    harness.click(cx, entry).unwrap();
}

/// Starts a thread by clicking a project's `+` button, tagged `new_thread`, and waits for its
/// agent to connect.
pub(crate) fn start_thread(
    harness: &DesktopHarness,
    cx: &mut TestAppContext,
    new_thread: &'static str,
) -> Entity<Thread> {
    let thread = start_thread_in(harness, cx, new_thread);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread
}

pub(crate) fn start_thread_in(
    harness: &DesktopHarness,
    cx: &mut TestAppContext,
    new_thread: &'static str,
) -> Entity<Thread> {
    harness.click(cx, new_thread).unwrap();
    harness.selected_thread(cx).expect("the new thread is selected")
}

/// An agent served over a WebSocket on a free local port, for one client.
pub(crate) struct WebSocketServer {
    pub(crate) url: String,
    /// The headers of the client's handshake, once it connects.
    pub(crate) headers: oneshot::Receiver<HeaderMap>,
}

/// Serves `agent` over a WebSocket until its one client hangs up.
#[allow(clippy::result_large_err, reason = "tungstenite's handshake callback returns its error response unboxed")]
pub(crate) async fn serve_websocket(agent: FakeAgent) -> WebSocketServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (headers_tx, headers) = oneshot::channel();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let socket = accept_hdr_async(stream, |request: &Request, response: Response| {
            let _ = headers_tx.send(request.headers().clone());
            Ok(response)
        })
        .await
        .unwrap();
        let _ = agent.agent().connect_to(WebSocketTransport::new(socket)).await;
    });
    WebSocketServer { url, headers }
}

/// Refuses every WebSocket handshake with `status`, returning the URL it listens at.
pub(crate) async fn refuse_websocket(status: StatusCode) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            refuse(stream, status).await;
        }
    });
    url
}

/// Serves `agent` over a WebSocket once it has refused `refusals` handshakes with 502 Bad Gateway,
/// as a proxy does while the server behind it is starting. Returns the URL it listens at.
pub(crate) async fn serve_websocket_once_started(agent: FakeAgent, refusals: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        for _ in 0..refusals {
            let (stream, _) = listener.accept().await.unwrap();
            refuse(stream, StatusCode::BAD_GATEWAY).await;
        }
        let (stream, _) = listener.accept().await.unwrap();
        let socket = accept_async(stream).await.unwrap();
        let _ = agent.agent().connect_to(WebSocketTransport::new(socket)).await;
    });
    url
}

/// Writes `config` as the `acp.json` file of the project at `root`.
pub(crate) fn write_agents_file(root: &Path, config: &serde_json::Value) {
    let file = project_agents_file(root);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, config.to_string()).unwrap();
}

/// Defines the agent `name` as `config` in the project at `root`, and reads it back as the app
/// would offer it.
pub(crate) fn define_agent(root: &Path, name: &str, config: &serde_json::Value) -> AgentProfile {
    write_agents_file(root, &json!({ "agents": { name: config } }));
    Agents::new(AgentCommand::new("aether"), None).catalog(root).get(name).cloned().expect("the agent is defined")
}

#[allow(clippy::result_large_err, reason = "tungstenite's handshake callback returns its error response unboxed")]
async fn refuse(stream: TcpStream, status: StatusCode) {
    let _ = accept_hdr_async(stream, |_: &Request, _: Response| {
        let mut error = ErrorResponse::new(None);
        *error.status_mut() = status;
        Err(error)
    })
    .await;
}
