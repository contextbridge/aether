#![cfg(target_family = "wasm")]

use acp_utils::client::{AcpClient, AcpClientError, AcpEvent, connect_acp_client};
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::initialize_request;
use acp_wasm::websocket::{CloseStatus, WebSocketClose, WebSocketError, connect_websocket};
use agent_client_protocol::schema::v2::PromptRequest;
use wasm_bindgen_test::wasm_bindgen_test;

const FAKE_AGENT_WS_URL: &str = env!("FAKE_AGENT_WS_URL");
const PROTOCOL: &str = "fake-agent.v1";
const ABNORMAL_CLOSURE: u16 = 1006;

#[wasm_bindgen_test]
async fn text_frames_carry_json_rpc_after_the_socket_opens() {
    let (client, _) = connect("/basic", &[]).await;
    let client = client.expect("initialize over the browser socket");
    let remote = RemoteServerInfo::from_meta(client.initialize_response.meta.as_ref());
    assert_eq!(remote, Some(RemoteServerInfo { cwd: "/fake/workspace".into(), session_id: None }));
}

#[wasm_bindgen_test]
async fn opening_an_unused_port_fails_with_an_abnormal_close() {
    let result = connect_websocket("ws://127.0.0.1:1", &[]).await;
    assert!(matches!(result, Err(WebSocketError::Closed(WebSocketClose { code: ABNORMAL_CLOSURE, .. }))));
}

#[wasm_bindgen_test]
async fn malformed_url_is_rejected_before_connecting() {
    let result = connect_websocket("not a websocket url", &[]).await;
    assert!(matches!(result, Err(WebSocketError::InvalidRequest(_))));
}

#[wasm_bindgen_test]
async fn offered_protocols_reach_the_handshake() {
    let (client, _) = connect("/protocol", &[PROTOCOL.into()]).await;
    client.expect("the server accepts the offered subprotocol");
}

#[wasm_bindgen_test]
async fn handshake_rejected_for_a_missing_protocol_fails_to_open() {
    let result = connect_websocket(&url("/protocol"), &[]).await;
    assert!(matches!(result, Err(WebSocketError::Closed(WebSocketClose { code: ABNORMAL_CLOSURE, .. }))));
}

#[wasm_bindgen_test]
async fn binary_frame_fails_the_connection() {
    let (client, _) = connect("/binary", &[]).await;
    assert!(client.is_err(), "a binary frame must not be silently dropped");
}

#[wasm_bindgen_test]
async fn close_before_initialize_records_the_servers_close() {
    let (client, close) = connect("/attached", &[]).await;
    assert!(client.is_err(), "the server closes instead of initializing");
    assert_eq!(close.get(), Some(WebSocketClose { code: 4409, reason: "another client is attached".into() }));
}

#[wasm_bindgen_test]
async fn clean_server_close_ends_the_connection_and_records_the_close() {
    let (client, close) = connect("/close", &[]).await;
    let mut client = client.expect("initialize over the browser socket");
    let prompt = client.handle.prompt(PromptRequest::new("fake-session", vec![]));
    assert!(prompt.await.is_err(), "the server closes instead of answering");
    assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
    assert!(client.event_rx.recv().await.is_none());
    assert_eq!(close.get(), Some(WebSocketClose { code: 4000, reason: "closed by the fake agent".into() }));
}

#[wasm_bindgen_test]
async fn client_disconnect_records_no_close() {
    let (client, close) = connect("/basic", &[]).await;
    let mut client = client.expect("initialize over the browser socket");
    client.handle.disconnect().await;
    assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
    assert_eq!(close.get(), None);
}

async fn connect(scenario: &str, protocols: &[String]) -> (Result<AcpClient, AcpClientError>, CloseStatus) {
    let (transport, close) = connect_websocket(&url(scenario), protocols).await.expect("socket opens");
    (connect_acp_client(transport, initialize_request()).await, close)
}

fn url(scenario: &str) -> String {
    format!("{FAKE_AGENT_WS_URL}{scenario}")
}
