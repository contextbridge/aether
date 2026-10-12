use mcp_utils::client::{ClientOptions, McpClient, Transport};
use mcp_utils::server::{McpServer, ServerHandle, UnixSocket};
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse, call};
use serde_json::json;
use std::fs::metadata;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn serve_unix_binds_a_socket_in_a_private_directory() {
    let handle = McpServer::new(FakeMcpServer::new()).serve_unix(UnixSocket::bind().unwrap());

    let directory = handle.path().parent().unwrap();
    assert!(handle.path().exists());
    assert_eq!(metadata(directory).unwrap().permissions().mode() & 0o777, 0o700);
}

#[tokio::test]
async fn a_socket_is_bound_before_a_server_is_attached_to_it() {
    let socket = UnixSocket::bind().unwrap();
    let path = socket.path().to_path_buf();
    assert!(path.exists());

    let handle = McpServer::new(FakeMcpServer::math()).serve_unix(socket);
    let tools = connect(&handle).await.list_tools().await.unwrap();

    assert_eq!(handle.path(), path);
    assert_eq!(tools.len(), 2);
}

#[tokio::test]
async fn dropping_the_handle_removes_the_socket_and_its_directory() {
    let handle = McpServer::new(FakeMcpServer::new()).serve_unix(UnixSocket::bind().unwrap());
    let socket = handle.path().to_path_buf();
    let directory = socket.parent().unwrap().to_path_buf();

    drop(handle);

    assert!(!socket.exists());
    assert!(!directory.exists());
}

#[tokio::test]
async fn stopping_the_handle_removes_the_socket_and_ends_open_connections() {
    let handle = McpServer::new(FakeMcpServer::new()).serve_unix(UnixSocket::bind().unwrap());
    let client = connect(&handle).await;

    handle.stop();

    assert!(!handle.path().exists());
    assert!(!handle.path().parent().unwrap().exists());
    assert!(client.list_tools().await.is_err());
}

#[tokio::test]
async fn unix_clients_reach_the_same_server() {
    let server = FakeMcpServer::math();
    let state = server.state();
    let handle = McpServer::new(server).serve_unix(UnixSocket::bind().unwrap());
    let first = connect(&handle).await;
    let second = connect(&handle).await;

    call(&first, "add_numbers", json!({"a": 1, "b": 2})).await.unwrap();
    call(&second, "add_numbers", json!({"a": 3, "b": 4})).await.unwrap();

    assert_eq!(state.calls_for("add_numbers").len(), 2);
}

#[tokio::test]
async fn dropping_the_handle_cancels_in_flight_calls() {
    let server = FakeMcpServer::new().with_tool(FakeTool::new("slow").responds(FakeToolResponse::pending()));
    let state = server.state();
    let handle = McpServer::new(server).serve_unix(UnixSocket::bind().unwrap());
    let client = connect(&handle).await;
    let in_flight = tokio::spawn(async move { call(&client, "slow", json!({})).await });
    state.wait_until(|state| !state.calls_for("slow").is_empty()).await;

    drop(handle);

    assert!(in_flight.await.unwrap().is_err());
}

#[tokio::test]
async fn closing_an_in_memory_client_stops_its_server() {
    let server = FakeMcpServer::new();
    let alive = server.alive();
    let client =
        McpClient::connect("test", Transport::InProcess(server.into()), &ClientOptions::default()).await.unwrap();

    client.close().await;

    while alive.upgrade().is_some() {
        tokio::task::yield_now().await;
    }
}

async fn connect(handle: &ServerHandle) -> McpClient {
    McpClient::connect("test", Transport::Unix(handle.path().to_path_buf()), &ClientOptions::default()).await.unwrap()
}
