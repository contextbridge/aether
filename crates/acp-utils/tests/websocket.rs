#![cfg(feature = "websocket")]

use acp_utils::client::{AcpEvent, connect_acp_client};
use acp_utils::websocket::WebSocketTransport;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, InitializeRequest, InitializeResponse,
    PromptRequest, PromptResponse, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{self as acp, Agent, Builder, Client, HandleDispatchFrom, NullRun};
use futures::{SinkExt, StreamExt};
use tokio::io::{DuplexStream, duplex};
use tokio::net::TcpListener;
use tokio::task::{LocalSet, spawn_local};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue;
use tokio_tungstenite::tungstenite::protocol::{
    Role, WebSocketConfig,
    frame::{
        Frame,
        coding::{Data, OpCode},
    },
};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async_with_config, connect_async};

#[tokio::test]
async fn acp_over_websocket_with_elicitation() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let (server, client) = SocketPairBuilder::default().build().await;
            let agent = streaming_elicitation_agent(&["hello ", "world"]);
            let server_task = spawn_local(agent.connect_to(WebSocketTransport::new(server)));
            let mut client =
                connect_acp_client(WebSocketTransport::new(client), InitializeRequest::new(ProtocolVersion::V1))
                    .await?;

            let handle = client.handle.clone();
            let prompt = spawn_local(async move {
                handle.prompt(PromptRequest::new("session", vec![ContentBlock::Text(TextContent::new("hi"))])).await
            });

            for expected in ["hello ", "world"] {
                let Some(AcpEvent::SessionUpdate { update, .. }) = client.event_rx.recv().await else {
                    return Err(TestError::Unexpected("expected streamed update"));
                };
                let SessionUpdate::AgentMessageChunk(chunk) = *update else {
                    return Err(TestError::Unexpected("expected agent message chunk"));
                };
                assert_eq!(chunk.content, ContentBlock::Text(TextContent::new(expected)));
            }

            let Some(AcpEvent::ElicitationRequest { responder, .. }) = client.event_rx.recv().await else {
                return Err(TestError::Unexpected("expected elicitation request"));
            };

            responder.respond(CreateElicitationResponse::new(ElicitationAction::Decline))?;
            assert_eq!(prompt.await??.stop_reason, StopReason::EndTurn);
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::PromptCompleted(StopReason::EndTurn))));
            drop(client);
            server_task.abort();
            let _ = server_task.await;
            Ok(())
        })
        .await
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn caller_established_socket_supports_acp_initialization() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(Box::pin(async {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let endpoint = format!("ws://{}/custom/path?session=example", listener.local_addr()?);
            let server = spawn_local(async move {
                let (stream, _) = listener.accept().await?;
                let socket = accept_hdr_async_with_config(
                    stream,
                    |request: &Request, response| {
                        assert_eq!(request.uri().path_and_query().unwrap().as_str(), "/custom/path?session=example");
                        assert_eq!(request.headers()["origin"], "https://example.com");
                        assert_eq!(request.headers()["x-aws-proxy-auth"], "secret");
                        assert_eq!(request.headers()["x-aws-proxy-port"], "8081");
                        assert_eq!(
                            request.headers().get_all("x-custom").iter().collect::<Vec<_>>(),
                            vec!["first", "second"]
                        );
                        assert_eq!(request.headers()["x-empty"], "");
                        Ok(response)
                    },
                    None,
                )
                .await?;
                test_agent().connect_to(WebSocketTransport::new(socket)).await.map_err(TestError::from)
            });
            let mut request = endpoint.into_client_request()?;
            request.headers_mut().insert("x-aws-proxy-auth", "secret".parse()?);
            request.headers_mut().insert("x-aws-proxy-port", "8081".parse()?);
            request.headers_mut().insert("origin", "https://example.com".parse()?);
            request.headers_mut().append("x-custom", "first".parse()?);
            request.headers_mut().append("x-custom", "second".parse()?);
            request.headers_mut().insert("x-empty", "".parse()?);
            let (socket, _) = connect_async(request).await?;
            let client = Client
                .builder()
                .connect_with(WebSocketTransport::new(socket), async |cx| {
                    let response = cx.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
                    assert_eq!(response.protocol_version, ProtocolVersion::V1);
                    Ok(())
                })
                .await;
            client?;
            let error = server.await?.expect_err("SDK foreground completion drops the socket");
            assert!(format!("{error:?}").contains("without closing handshake"), "{error:?}");
            Ok(())
        }))
        .await
}

#[tokio::test]
async fn final_response_is_not_lost_when_close_is_already_buffered() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let (mut server, client_socket) = SocketPairBuilder::default().build().await;
            let peer = spawn_local(async move {
                let request = receive_json(&mut server).await?;
                let response =
                    serde_json::json!({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": 1}});
                server.feed(Message::Text(response.to_string().into())).await?;
                server.feed(Message::Close(None)).await?;
                server.flush().await?;
                while server
                    .next()
                    .await
                    .is_some_and(|message| matches!(message, Ok(Message::Ping(_) | Message::Pong(_))))
                {}
                Ok::<_, TestError>(())
            });
            let client =
                connect_acp_client(WebSocketTransport::new(client_socket), InitializeRequest::new(ProtocolVersion::V1))
                    .await;
            assert!(client.is_ok(), "last response must be delivered before EOF");
            peer.await??;
            Ok(())
        })
        .await
}

#[tokio::test]
async fn invalid_utf8_terminates_the_connection() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let (server, mut client) = SocketPairBuilder::default().build().await;
            let task = spawn_local(Agent.builder().connect_to(WebSocketTransport::new(server)));
            client.send(Message::Frame(Frame::message(vec![0xff], OpCode::Data(Data::Text), true))).await?;
            assert!(task.await?.is_err());
            Ok(())
        })
        .await
}

#[derive(Default)]
struct SocketPairBuilder {
    server_config: Option<WebSocketConfig>,
    client_config: Option<WebSocketConfig>,
}

impl SocketPairBuilder {
    async fn build(self) -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
        let (server, client) = duplex(64 * 1024);
        tokio::join!(
            WebSocketStream::from_raw_socket(server, Role::Server, self.server_config),
            WebSocketStream::from_raw_socket(client, Role::Client, self.client_config),
        )
    }
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error(transparent)]
    Client(#[from] acp_utils::client::AcpClientError),
    #[error(transparent)]
    Acp(#[from] acp::Error),
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Header(#[from] InvalidHeaderValue),
    #[error("{0}")]
    Unexpected(&'static str),
}

async fn receive_message(socket: &mut WebSocketStream<DuplexStream>) -> Result<Message, TestError> {
    Ok(socket.next().await.ok_or(TestError::Unexpected("socket closed before expected message"))??)
}

async fn receive_json(socket: &mut WebSocketStream<DuplexStream>) -> Result<serde_json::Value, TestError> {
    let Message::Text(text) = receive_message(socket).await? else {
        return Err(TestError::Unexpected("expected JSON text message"));
    };
    Ok(serde_json::from_str(&text)?)
}

fn test_agent() -> Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
    Agent.builder().on_receive_request(
        async |_: InitializeRequest, responder, _cx| responder.respond(InitializeResponse::new(ProtocolVersion::V1)),
        acp::on_receive_request!(),
    )
}

fn streaming_elicitation_agent(
    chunks: &'static [&'static str],
) -> Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
    test_agent().on_receive_request(
        async move |request: PromptRequest, responder, cx| {
            for &text in chunks {
                cx.send_notification(SessionNotification::new(
                    request.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(text)))),
                ))?;
            }
            let input = CreateElicitationRequest::new(
                ElicitationFormMode::new(ElicitationSessionScope::new(request.session_id), ElicitationSchema::new()),
                "Continue?",
            );
            let connection = cx.clone();
            cx.spawn(async move {
                let response = connection.send_request(input).block_task().await?;
                assert_eq!(response.action, ElicitationAction::Decline);
                responder.respond(PromptResponse::new(StopReason::EndTurn))
            })?;
            Ok(())
        },
        acp::on_receive_request!(),
    )
}
