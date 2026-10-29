use acp_utils::testing::{FakeAgent, Question};
use acp_utils::websocket::WebSocketTransport;
use aether_desktop::agents::{AgentProfile, Agents, CommandLine, project_agents_file};
use aether_desktop::testing::DesktopHarness;
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2::{ElicitationSchema, StringPropertySchema};
use clankerdiff_server::{DiffServer, ServerListener, ServerOptions};
use gpui::{Entity, TestAppContext};
use serde_json::json;
use std::fs;
use std::path::Path;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderMap, StatusCode};

pub(crate) fn replying(session_id: &str, reply: &str) -> FakeAgent {
    FakeAgent::new(session_id).prompt_reply(reply)
}

pub(crate) fn texts(thread: &Thread) -> Vec<String> {
    thread.core().map(|core| acp_utils::testing::texts(core.conversation())).unwrap_or_default()
}

pub(crate) fn asking_name() -> FakeAgent {
    FakeAgent::new("alpha-session").prompt_asks("Who are you?", Question::Form(name_form()))
}

pub(crate) fn name_form() -> ElicitationSchema {
    ElicitationSchema::new().property("name", StringPropertySchema::new().title("Name"), true)
}

pub(crate) fn asked(cx: &mut TestAppContext, agent: FakeAgent) -> (DesktopHarness, Entity<Thread>) {
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    thread.update(cx, |thread, cx| thread.send("go".into(), cx));
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::NeedsInput);
    (harness, thread)
}

pub(crate) fn choose(harness: &DesktopHarness, cx: &mut TestAppContext, row: &'static str, entry: &'static str) {
    harness.right_click(cx, row).unwrap();
    harness.click(cx, entry).unwrap();
}

pub(crate) struct WebSocketServer {
    pub(crate) url: String,
    pub(crate) headers: oneshot::Receiver<HeaderMap>,
}

pub(crate) async fn serve_websocket(agent: FakeAgent) -> WebSocketServer {
    serve_websocket_refusing(agent, 0, StatusCode::OK).await
}

#[allow(clippy::result_large_err, reason = "tungstenite's handshake callback returns its error response unboxed")]
pub(crate) async fn serve_websocket_refusing(agent: FakeAgent, refusals: usize, status: StatusCode) -> WebSocketServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (headers_tx, headers) = oneshot::channel();
    tokio::spawn(async move {
        for _ in 0..refusals {
            let (stream, _) = listener.accept().await.unwrap();
            refuse(stream, status).await;
        }
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

pub(crate) struct ServedDiff {
    pub(crate) url: String,
    _listener: ServerListener,
    _server: DiffServer,
    _runtime: Runtime,
}

pub(crate) fn serve_diff(root: &Path) -> ServedDiff {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let (server, listener) = runtime.block_on(async {
        let server = DiffServer::open(root, ServerOptions::default()).await.unwrap();
        let listener = server.listen(([127, 0, 0, 1], 0).into()).await.unwrap();
        (server, listener)
    });
    ServedDiff {
        url: format!("ws://{}/ws", listener.local_addr()),
        _listener: listener,
        _server: server,
        _runtime: runtime,
    }
}

pub(crate) fn write_agents_file(root: &Path, config: &serde_json::Value) {
    let file = project_agents_file(root);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, config.to_string()).unwrap();
}

pub(crate) fn define_agent(root: &Path, name: &str, config: &serde_json::Value) -> AgentProfile {
    write_agents_file(root, &json!({ "agents": { name: config } }));
    Agents::new(CommandLine::new("aether"), None).catalog(root).get(name).cloned().expect("the agent is defined")
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
