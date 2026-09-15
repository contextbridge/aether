use acp_utils::websocket::WebSocketTransport;
use aether_cli::acp::testing::AcpTestHarness;
use aether_cli::client::{ClientArgs, ClientRunError, run_client};
use agent_client_protocol::schema::v2::{CloseSessionRequest, PromptRequest};
use clap::Parser;
use tokio_tungstenite::tungstenite::handshake::server::Request;

#[derive(Parser)]
struct ClientCli {
    #[command(flatten)]
    args: ClientArgs,
}

#[test]
fn remote_client_parser_defaults_and_overrides() {
    let args = ClientCli::try_parse_from(["client"]).unwrap().args;
    assert_eq!(args.url, "ws://127.0.0.1:8765");
    assert!(args.session.is_none());
    assert!(args.headers.is_empty());
    assert!(args.log_dir.is_none());
    let args = ClientCli::try_parse_from([
        "client",
        "wss://example.com/acp?target=build",
        "--session",
        "saved",
        "-H",
        "X-Test: one:two",
        "--header",
        "X-Test: three",
        "--log-dir",
        "/tmp/client-logs",
    ])
    .unwrap()
    .args;
    assert_eq!(args.session.as_deref(), Some("saved"));
    assert_eq!(args.log_dir.as_deref(), Some(std::path::Path::new("/tmp/client-logs")));
    let request = args.connection_request().unwrap();
    assert_eq!(request.uri().scheme_str(), Some("wss"));
    assert_eq!(request.uri().path_and_query().unwrap().as_str(), "/acp?target=build");
    let values: Vec<_> = request.headers().get_all("x-test").iter().collect();
    assert_eq!(values.len(), 2);
    assert_eq!(values[0], "one:two");
    assert_eq!(values[1], "three");
    assert!(values.iter().all(|value| value.is_sensitive()));
}

#[tokio::test]
async fn remote_client_rejects_invalid_inputs_before_connecting() {
    for url in ["http://127.0.0.1:1", "https://example.com", "file:///tmp/acp", "not a url", "ws://"] {
        let args = ClientCli::try_parse_from(["client", url]).unwrap().args;
        assert!(matches!(run_client(args).await, Err(ClientRunError::InvalidUrl)));
    }
    for header in ["no-colon", "Bad Name: value", ": value", "X-Test: value\r\nInjected: true"] {
        let args = ClientCli::try_parse_from(["client", "ws://127.0.0.1:1", "-H", header]).unwrap().args;
        assert!(matches!(run_client(args).await, Err(ClientRunError::InvalidHeader { index: 1 })));
    }
}

#[tokio::test]
async fn remote_client_reports_occupied_server_without_disturbing_owner() {
    AcpTestHarness::run(|mut harness| async move {
        let inserted = harness.insert_agent_switching_session().await;
        let id = inserted.session_id().clone();
        let server = harness.serve_websocket().await;
        let args = ClientCli::try_parse_from(["client", &server.url()]).unwrap().args;
        let (socket, _) = tokio_tungstenite::connect_async(args.connection_request().unwrap()).await.unwrap();
        let session = wisp::Session::connect_remote_to(WebSocketTransport::new(socket), None).await.unwrap();
        assert_eq!(session.response.session_id, id);
        assert!(matches!(run_client(args).await, Err(ClientRunError::ServerOccupied)));
        session.client.handle.prompt(PromptRequest::new(id.clone(), vec!["hello".into()])).await.unwrap();
        let mut detached = server.subscribe();
        let seen = *detached.borrow();
        session.client.handle.disconnect().await;
        detached.wait_for(|g| *g > seen).await.unwrap();
        let (socket, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
        let session = wisp::Session::connect_remote_to(WebSocketTransport::new(socket), None).await.unwrap();
        assert_eq!(session.response.session_id, id);
        let mut detached = server.subscribe();
        let seen = *detached.borrow();
        session.client.handle.disconnect().await;
        detached.wait_for(|g| *g > seen).await.unwrap();
        server.shutdown().await;
        harness.shutdown().await;
    })
    .await;
}

#[tokio::test]
async fn remote_client_fresh_startup_then_live_and_saved_resume() {
    AcpTestHarness::run(|mut harness| async move {
        let server = harness.serve_websocket().await;
        let (socket, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
        let session = wisp::Session::connect_remote_to(WebSocketTransport::new(socket), None).await.unwrap();
        let id = session.response.session_id.clone();
        assert_eq!(session.working_dir, std::path::PathBuf::from("/tmp"));
        let mut detached = server.subscribe();
        let seen = *detached.borrow();
        session.client.handle.disconnect().await;
        detached.wait_for(|g| *g > seen).await.unwrap();
        for saved in [false, true] {
            let (socket, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
            let session = wisp::Session::connect_remote_to(WebSocketTransport::new(socket), saved.then(|| id.clone()))
                .await
                .unwrap();
            assert_eq!(session.response.session_id, id);
            if !saved {
                session.client.handle.request(CloseSessionRequest::new(id.clone())).await.unwrap();
            }
            let mut detached = server.subscribe();
            let seen = *detached.borrow();
            session.client.handle.disconnect().await;
            detached.wait_for(|g| *g > seen).await.unwrap();
        }
        server.shutdown().await;
        harness.shutdown().await;
    })
    .await;
}

#[tokio::test]
#[expect(clippy::result_large_err, reason = "tungstenite's handshake callback requires an HTTP error response")]
async fn remote_client_handshake_transmits_headers_path_and_query() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/acp?target=build", listener.local_addr().unwrap());
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        tokio_tungstenite::accept_hdr_async(socket, |request: &Request, response| {
            assert_eq!(request.uri().path_and_query().unwrap().as_str(), "/acp?target=build");
            assert_eq!(request.headers().get_all("x-test").iter().count(), 2);
            assert!(request.headers().get_all("x-test").iter().any(|value| value == "one:two"));
            Ok(response)
        })
        .await
        .unwrap()
    });
    let args =
        ClientCli::try_parse_from(["client", &url, "-H", "X-Test: one:two", "-H", "X-Test: three"]).unwrap().args;
    let (_socket, _) = tokio_tungstenite::connect_async(args.connection_request().unwrap()).await.unwrap();
    let _peer = peer.await.unwrap();
}

#[tokio::test]
async fn remote_client_handshake_errors_do_not_expose_headers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/acp?target=build", listener.local_addr().unwrap());
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut socket, b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    let args = ClientCli::try_parse_from(["client", &url, "-H", "Authorization: Bearer private-token"]).unwrap().args;
    let error = run_client(args).await.unwrap_err();
    assert!(matches!(error, ClientRunError::Handshake));
    assert_eq!(format!("{error:?}"), "Handshake");
    peer.await.unwrap();
}
