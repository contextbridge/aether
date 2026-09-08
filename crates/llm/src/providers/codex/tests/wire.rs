use super::test_server::{FakeResponsesWebsocketServer, Reply, text_events};
use super::{MODEL, collect, connection, context, provider, provider_at};
use crate::providers::codex::CodexProvider;
use crate::{LlmResponse, StreamingModelProvider};
use aether_auth::FakeOAuthCredentialStore;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::test]
async fn websocket_wire_contract() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    let mut context = context("conversation-1");
    context.set_system_content("You are helpful".into());
    context.set_tools(vec![crate::ToolDefinition::new(
        "bash",
        "Run a command",
        serde_json::json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
    )]);
    context.set_reasoning_effort(Some(crate::ReasoningEffort::Max));
    context.set_prompt_cache_key(Some("cache-1".into()));
    let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
    assert!(responses.iter().all(Result::is_ok), "{responses:?}");
    assert!(matches!(responses.last(), Some(Ok(LlmResponse::Done { .. }))));
    let request = server.captured().await;
    assert_eq!(request.uri.path(), "/responses");
    assert_eq!(request.body["type"], "response.create");
    assert_eq!(request.body["stream"], true);
    assert_eq!(request.body["store"], false);
    assert_eq!(request.body["model"], "gpt-5.6-luna");
    assert_eq!(request.body["reasoning"]["effort"], "max");
    assert!(request.body["reasoning"].get("context").is_none());
    assert_eq!(request.body["instructions"], "You are helpful");
    assert_eq!(
        request.body["tools"],
        serde_json::json!([{
            "type":"function", "name":"bash", "description":"Run a command",
            "parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}
        }])
    );
    assert_eq!(request.body["input"][0]["role"], "user");
    assert_eq!(request.body["prompt_cache_key"], "cache-1");
    assert!(request.body.get("parallel_tool_calls").is_none());
    assert_eq!(request.headers["version"], "0.153.4");
    assert_eq!(request.headers["originator"], "codex_cli_rs");
    assert!(!request.headers.contains_key("x-openai-internal-codex-responses-lite"));
    assert_eq!(request.headers["openai-beta"], "responses_websockets=2026-02-06");
    assert_eq!(request.headers["chatgpt-account-id"], "account-1");
    assert_eq!(request.headers["session-id"], request.headers["thread-id"]);
    assert!(request.headers.contains_key("x-client-request-id"));
    assert!(request.body.get("previous_response_id").is_none());
}

#[tokio::test]
async fn default_wire_settings_and_display_name() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    collect(&provider, &context("same")).await;
    assert_eq!(server.captured().await.body["reasoning"]["effort"], "medium");
    assert_eq!(provider.display_name(), "Codex (gpt-5.6-luna)");
    assert_eq!(provider.context_window(), Some(272_000));
}

#[tokio::test]
async fn routing_token_is_first_wins_and_scoped_to_turn() {
    let mut events = text_events("one", "Hello");
    events.insert(1, serde_json::json!({"type":"response.metadata","headers":{"X-Codex-Turn-State":[["first"]]}}));
    events.insert(2, serde_json::json!({"type":"response.metadata","headers":{"x-codex-turn-state":"second"}}));
    let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
    let provider = provider(&server);
    let mut context = context("same");
    for turn in ["turn-1", "turn-1", "turn-2"] {
        context.set_turn_id(Some(turn.into()));
        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
    }
    let first = server.captured().await;
    let second = server.captured().await;
    let third = server.captured().await;
    assert_eq!(first.connection, third.connection);
    assert_eq!(second.body["client_metadata"]["x-codex-turn-state"], "first");
    assert!(third.body["client_metadata"].get("x-codex-turn-state").is_none());
}

#[tokio::test]
async fn handshake_token_survives_same_turn_reconnect_but_not_new_turn() {
    let rejection =
        serde_json::json!({"type":"error","error":{"code":"previous_response_not_found","message":"expired"}});
    let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(vec![rejection])]).await;
    server.handshake_token("first-token");
    let provider = provider(&server);
    let mut context = context("same");
    collect(&provider, &context).await;
    let first = server.captured().await;
    let second = server.captured().await;
    assert_ne!(first.connection, second.connection);
    assert_eq!(second.body["client_metadata"]["x-codex-turn-state"], "first-token");
    context.set_turn_id(Some("next-turn".into()));
    collect(&provider, &context).await;
    let third = server.captured().await;
    assert_eq!(third.connection, second.connection);
    assert!(third.body["client_metadata"].get("x-codex-turn-state").is_none());
}

#[tokio::test]
async fn no_turn_id_does_not_replay_routing_tokens() {
    let mut events = text_events("one", "Hello");
    events.insert(1, serde_json::json!({"type":"response.metadata","headers":{"x-codex-turn-state":"token"}}));
    let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
    let provider = provider(&server);
    let mut context = context("same");
    context.set_turn_id(None);
    collect(&provider, &context).await;
    collect(&provider, &context).await;
    let first = server.captured().await;
    let second = server.captured().await;
    assert_eq!(first.connection, second.connection);
    assert_ne!(first.body["client_metadata"]["x-codex-turn-id"], second.body["client_metadata"]["x-codex-turn-id"]);
    assert!(second.body["client_metadata"].get("x-codex-turn-state").is_none());
}

#[tokio::test]
async fn base_paths_and_queries_are_preserved_and_invalid_urls_rejected() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    collect(&provider_at(&format!("{}/base/?key=value", server.base_url)), &context("same")).await;
    let captured = server.captured().await;
    assert_eq!(captured.uri.path(), "/base/responses");
    assert_eq!(captured.uri.query(), Some("key=value"));
    let store = Arc::new(FakeOAuthCredentialStore::new());
    for url in
        ["ws://localhost", "https://user:password@localhost", "https://localhost/#fragment", "not-url", "file:///path"]
    {
        let provider = CodexProvider::new(store.clone(), connection(url), MODEL);
        assert!(matches!(provider, Err(crate::LlmError::ProviderRequest(_))), "{url}");
    }
}
