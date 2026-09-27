use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::{FakeAgent, initialize_response};
use acp_utils::websocket::WebSocketTransport;
use agent_client_protocol::schema::v2::NewSessionResponse;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::error::Error;
use std::io;
use std::process::ExitCode;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::task::{LocalSet, spawn_local};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async};

const SESSION_ID: &str = "fake-session";
const REPLY: &str = "hello from the fake agent";
const HISTORY: &str = "saved answer";
const REMOTE_CWD: &str = "/fake/workspace";
const PROTOCOL: &str = "fake-agent.v1";
const ATTACHED_CODE: u16 = 4409;
const ATTACHED_REASON: &str = "another client is attached";
const CLOSE_CODE: u16 = 4000;
const CLOSE_REASON: &str = "closed by the fake agent";
const DEFAULT_PORT: u16 = 8765;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<ExitCode, Box<dyn Error>> {
    let mut command = std::env::args().skip(1);
    let program = command.next();
    let port = if program.is_some() { 0 } else { DEFAULT_PORT };
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let url = format!("ws://{}", listener.local_addr()?);
    LocalSet::new()
        .run_until(async move {
            spawn_local(accept_clients(listener));
            let Some(program) = program else {
                println!("fake agent listening on {url}");
                return Ok(std::future::pending().await);
            };
            let status = Command::new(program).args(command).env("FAKE_AGENT_WS_URL", url).status().await?;
            Ok(status.code().and_then(|code| u8::try_from(code).ok()).map_or(ExitCode::FAILURE, ExitCode::from))
        })
        .await
}

async fn accept_clients(listener: TcpListener) -> io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        spawn_local(async move {
            if let Err(error) = serve(stream).await {
                eprintln!("fake agent: {error}");
            }
        });
    }
}

#[allow(clippy::result_large_err)]
async fn serve(stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let mut path = String::new();
    let mut socket = accept_hdr_async(stream, |request: &Request, mut response: Response| {
        request.uri().path().clone_into(&mut path);
        if path == "/protocol" {
            if !offers_protocol(request) {
                let mut error = ErrorResponse::new(None);
                *error.status_mut() = StatusCode::UNAUTHORIZED;
                return Err(error);
            }
            response.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(PROTOCOL));
        }
        Ok(response)
    })
    .await?;
    match path.as_str() {
        "/" | "/basic" | "/protocol" => {
            socket.send(Message::Ping(Bytes::new())).await?;
            agent().prompt_reply(REPLY).agent().connect_to(WebSocketTransport::new(socket)).await?;
        }
        "/elicitation" => {
            agent().prompt_elicitation("Continue?").agent().connect_to(WebSocketTransport::new(socket)).await?;
        }
        "/attached" => close(&mut socket, ATTACHED_CODE, ATTACHED_REASON).await?,
        "/close" => {
            // Answered by hand: the ACP transport owns the socket and can only close it without a code.
            let initialize = next_request(&mut socket).await?;
            let response = json!({"jsonrpc": "2.0", "id": initialize["id"], "result": initialize_response()});
            socket.send(Message::text(response.to_string())).await?;
            next_request(&mut socket).await?;
            close(&mut socket, CLOSE_CODE, CLOSE_REASON).await?;
        }
        "/binary" => {
            socket.send(Message::Binary(Bytes::from_static(b"binary"))).await?;
            agent().agent().connect_to(WebSocketTransport::new(socket)).await?;
        }
        _ => return Err(format!("unknown scenario: {path}").into()),
    }
    Ok(())
}

fn offers_protocol(request: &Request) -> bool {
    request
        .headers()
        .get_all(SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|protocol| protocol.trim() == PROTOCOL)
}

async fn next_request(socket: &mut WebSocketStream<TcpStream>) -> Result<serde_json::Value, Box<dyn Error>> {
    match socket.next().await {
        Some(Ok(Message::Text(text))) => Ok(serde_json::from_str(&text)?),
        Some(Err(error)) => Err(error.into()),
        _ => Err("expected a JSON-RPC request from the client".into()),
    }
}

async fn close(socket: &mut WebSocketStream<TcpStream>, code: u16, reason: &'static str) -> Result<(), Box<dyn Error>> {
    socket.close(Some(CloseFrame { code: code.into(), reason: reason.into() })).await?;
    while let Some(Ok(_)) = socket.next().await {}
    Ok(())
}

fn agent() -> FakeAgent {
    FakeAgent::default()
        .remote_server(&RemoteServerInfo { cwd: REMOTE_CWD.into(), session_id: None })
        .new_session_response(NewSessionResponse::new(SESSION_ID))
        .replay_message(SESSION_ID, HISTORY)
}
