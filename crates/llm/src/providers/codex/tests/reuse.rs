use super::test_server::{FakeResponsesWebsocketServer, Reply, text_events};
use super::{MODEL, collect, connection, context, credential, provider};
use crate::providers::codex::CodexProvider;
use crate::{LlmResponse, ProviderConnectionConfig, StreamingModelProvider};
use aether_auth::FakeOAuthCredentialStore;
use axum::extract::ws::Message;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::oneshot;

#[test]
fn provider_and_unpolled_stream_do_not_require_a_runtime() {
    let store = Arc::new(FakeOAuthCredentialStore::new().with_credential("codex", credential("account-1")));
    let provider = CodexProvider::new(store, ProviderConnectionConfig::default(), MODEL).unwrap();
    let stream = provider.stream_response(&context("lazy"));
    drop(provider);
    drop(stream);
}

#[tokio::test]
async fn one_provider_cannot_be_rebound_to_another_conversation() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    collect(&provider, &context("one")).await;
    let first = server.captured().await;
    let mut rejected = provider.stream_response(&context("two"));
    let error = rejected.next().await.unwrap().unwrap_err();
    assert!(!error.is_retryable());
    assert!(error.to_string().contains("conversation"));
    collect(&provider, &context("one")).await;
    assert_eq!(server.captured().await.connection, first.connection);
    assert!(rejected.next().await.is_none());
}

#[tokio::test]
async fn server_completion_does_not_release_session_before_consumer_done() {
    let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(text_events("one", "Hello"))]).await;
    let provider = provider(&server);
    let mut stream = provider.stream_response(&context("same"));
    assert!(matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Start { .. }));
    let first = server.captured().await;

    let mut queued = provider.clone().stream_response(&context("same"));
    assert!(futures::poll!(queued.next()).is_pending());
    drop(stream);
    assert!(queued.collect::<Vec<_>>().await.iter().all(Result::is_ok));
    assert_ne!(server.captured().await.connection, first.connection);
}

#[tokio::test]
async fn dropping_stream_closes_silent_socket_without_another_request() {
    let reply = Reply::events(vec![serde_json::json!({
        "type": "response.created", "response": {"id": "silent"}
    })]);
    let mut server = FakeResponsesWebsocketServer::start(vec![reply]).await;
    let provider = provider(&server);
    let mut stream = provider.stream_response(&context("same"));
    assert!(matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Start { .. }));
    let first = server.captured().await;
    drop(stream);
    assert_eq!(server.closed().await, first.connection);
}

#[tokio::test]
async fn completed_requests_reuse_socket_before_done_is_yielded() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    let context = context("same");
    let mut stream = provider.stream_response(&context);
    while !matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Done { .. }) {}
    let first = server.captured().await;
    let second = provider.clone().stream_response(&context).collect::<Vec<_>>().await;
    assert!(second.iter().all(Result::is_ok));
    assert_eq!(first.connection, server.captured().await.connection);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_stream_after_done_preserves_reuse() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    let mut first_connection = None;
    for _ in 0..16 {
        let mut stream = provider.stream_response(&context("same"));
        while !matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Done { .. }) {}
        drop(stream);
        let request = server.captured().await;
        assert_eq!(request.connection, *first_connection.get_or_insert(request.connection));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_usage_does_not_allow_reuse_until_done_is_consumed() {
    for consume_done in [false, true] {
        let mut events = text_events("one", "Hello");
        events[2]["response"]["usage"] = serde_json::json!({
            "input_tokens": 2, "input_tokens_details": {"cached_tokens": 0},
            "output_tokens": 1, "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": 3
        });
        let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
        let provider = provider(&server);
        let mut context = context("same");
        let mut stream = provider.stream_response(&context);
        while !matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Usage { .. }) {}
        let first = server.captured().await;
        context.push_assistant_turn("Hello", crate::AssistantReasoning::default(), vec![]);
        context.add_message(crate::ChatMessage::user("next"));
        let mut queued = provider.stream_response(&context);
        assert!(futures::poll!(queued.next()).is_pending());
        if consume_done {
            assert!(matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Done { .. }));
        }
        drop(stream);
        if !consume_done {
            assert_eq!(server.closed().await, first.connection);
        }
        assert!(queued.collect::<Vec<_>>().await.iter().all(Result::is_ok));
        let second = server.captured().await;
        assert_eq!(first.connection == second.connection, consume_done);
        assert_eq!(second.body.get("previous_response_id").is_some(), consume_done);
        assert_eq!(second.effective_input, super::full_history(&context));
    }
}

#[tokio::test]
async fn dropping_queued_requests_preserves_the_active_connection() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    let context = context("same");
    let mut active = provider.stream_response(&context);
    assert!(matches!(active.next().await.unwrap().unwrap(), LlmResponse::Start { .. }));
    let first = server.captured().await;

    let mut queued = provider.stream_response(&context);
    assert!(futures::poll!(queued.next()).is_pending());
    let mut backpressured = provider.stream_response(&context);
    assert!(futures::poll!(backpressured.next()).is_pending());
    drop(backpressured);
    drop(queued);

    let mut next = provider.stream_response(&context);
    assert!(futures::poll!(next.next()).is_pending());
    assert!(active.collect::<Vec<_>>().await.iter().all(Result::is_ok));
    assert!(next.collect::<Vec<_>>().await.iter().all(Result::is_ok));
    assert_eq!(server.captured().await.connection, first.connection);
}

#[tokio::test]
async fn dropping_pending_stream_releases_session() {
    let (release, wait) = oneshot::channel();
    let mut reply = Reply::events(text_events("one", "Hello"));
    reply.release = Some(wait);
    let mut server = FakeResponsesWebsocketServer::start(vec![reply]).await;
    let provider = provider(&server);
    let mut stream = provider.stream_response(&context("same"));
    let first = tokio::select! {
        first = server.captured() => first,
        response = stream.next() => panic!("server has not released a response: {response:?}"),
    };
    drop(stream);
    release.send(()).unwrap();
    assert_eq!(server.closed().await, first.connection);
    collect(&provider, &context("same")).await;
    assert_ne!(server.captured().await.connection, first.connection);
}

#[tokio::test]
async fn separate_providers_run_concurrently_without_sharing_sockets() {
    let (release, wait) = oneshot::channel();
    let mut reply = Reply::events(text_events("one", "Hello"));
    reply.release = Some(wait);
    let mut server = FakeResponsesWebsocketServer::start(vec![reply]).await;
    let provider = provider(&server);
    let other = self::provider(&server);
    let first = tokio::spawn(provider.stream_response(&context("one")).collect::<Vec<_>>());
    let one = server.captured().await;
    collect(&other, &context("two")).await;
    let two = server.captured().await;
    assert_ne!(one.connection, two.connection);
    assert_eq!(one.headers["session-id"], "one");
    assert_eq!(two.headers["session-id"], "two");
    release.send(()).unwrap();
    assert!(first.await.unwrap().iter().all(Result::is_ok));
    for (provider, key, connection) in [(&provider, "one", one.connection), (&other, "two", two.connection)] {
        collect(provider, &context(key)).await;
        assert_eq!(server.captured().await.connection, connection);
    }
}

#[tokio::test]
async fn stream_keeps_sessions_alive_after_original_provider_is_dropped() {
    let (release, wait) = oneshot::channel();
    let mut reply = Reply::events(text_events("one", "Hello"));
    reply.release = Some(wait);
    let mut server = FakeResponsesWebsocketServer::start(vec![reply]).await;
    let provider = provider(&server);
    let stream = tokio::spawn(provider.stream_response(&context("one")).collect::<Vec<_>>());
    let first = server.captured().await;
    drop(provider);
    release.send(()).unwrap();
    assert!(stream.await.unwrap().iter().all(Result::is_ok));
    assert_eq!(server.closed().await, first.connection);
}

#[tokio::test]
async fn unkeyed_requests_reuse_one_provider_session_and_stable_routing_id() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    collect(&provider, &context("")).await;
    let first = server.captured().await;
    collect(&provider, &context("")).await;
    let second = server.captured().await;
    assert_eq!(first.connection, second.connection);
    assert_eq!(first.headers["session-id"], second.headers["session-id"]);

    collect(&self::provider(&server), &context("")).await;
    let other = server.captured().await;
    assert_ne!(first.connection, other.connection);
    assert_ne!(first.headers["session-id"], other.headers["session-id"]);
}

#[tokio::test]
async fn dropping_stream_before_done_discards_connection() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    let context = context("same");
    let mut stream = provider.stream_response(&context);
    assert!(matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Start { .. }));
    let first = server.captured().await;
    drop(stream);
    collect(&provider, &context).await;
    assert_ne!(first.connection, server.captured().await.connection);
}

#[tokio::test]
async fn incomplete_errors_and_malformed_frames_never_reuse_connections() {
    let cases = vec![
        vec![Message::Text("not json".into())],
        vec![Message::Binary(vec![1, 2].into())],
        vec![Message::Close(None)],
        Reply::events(vec![serde_json::json!({"type":"response.output_text.delta"})]).frames,
        Reply::events(vec![serde_json::json!({"type":"response.output_text.delta","delta":"before created"})]).frames,
        Reply::events(vec![serde_json::json!({"type":"response.created","response":{"id":"one"}}), serde_json::json!({"type":"response.incomplete","response":{"status":"incomplete"}})]).frames,
        Reply::events(vec![serde_json::json!({"type":"response.failed","response":{"error":{"code":"server_error","message":"failed"}}})]).frames,
    ];
    for frames in cases {
        let mut server = FakeResponsesWebsocketServer::start(vec![Reply { frames, release: None }]).await;
        let provider = provider(&server);
        let context = context("same");
        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let incomplete = responses
            .iter()
            .any(|response| matches!(response, Ok(LlmResponse::Done { stop_reason: Some(crate::StopReason::Length) })));
        assert!(incomplete || responses.last().unwrap().is_err(), "{responses:?}");
        if !incomplete {
            assert!(!responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))));
        }
        collect(&provider, &context).await;
        assert_ne!(server.captured().await.connection, server.captured().await.connection);
    }
}

#[tokio::test]
async fn idle_socket_services_ping_and_provider_drop_closes_it() {
    let mut reply = Reply::events(text_events("one", "Hello"));
    reply.frames.push(Message::Ping(vec![1].into()));
    let mut server = FakeResponsesWebsocketServer::start(vec![reply]).await;
    let provider = provider(&server);
    collect(&provider, &context("same")).await;
    let request = server.captured().await;
    server.pong().await;
    drop(provider);
    assert_eq!(server.closed().await, request.connection);
}

#[tokio::test]
async fn idle_deadline_closes_socket_without_another_request() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    collect(&provider, &context("same")).await;
    let first = server.captured().await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(301)).await;
    assert_eq!(server.closed().await, first.connection);
    tokio::time::resume();
    collect(&provider, &context("same")).await;
    assert_ne!(server.captured().await.connection, first.connection);
}

#[tokio::test]
async fn max_socket_age_is_bounded() {
    let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
    let provider = provider(&server);
    collect(&provider, &context("same")).await;
    let first = server.captured().await;
    for iteration in 1..=14 {
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_mins(4)).await;
        tokio::time::resume();
        collect(&provider, &context("same")).await;
        let request = server.captured().await;
        if iteration < 14 {
            assert_eq!(request.connection, first.connection);
        } else {
            assert_ne!(request.connection, first.connection);
        }
    }
    assert_eq!(server.closed().await, first.connection);
}

#[tokio::test]
async fn active_event_deadline_and_drop_during_backpressure_release_socket() {
    let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(vec![
        serde_json::json!({"type":"response.created","response":{"id":"one"}}),
    ])])
    .await;
    let provider = provider(&server);
    let mut stream = provider.stream_response(&context("same"));
    assert!(matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Start { .. }));
    server.captured().await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(301)).await;
    assert_eq!(stream.next().await.unwrap().unwrap_err().provider().unwrap().kind, crate::ProviderErrorKind::Timeout);
    tokio::time::resume();
    drop(stream);
    server.closed().await;

    let mut events = vec![serde_json::json!({"type":"response.created","response":{"id":"flood"}})];
    events.extend((0..128).map(|_| serde_json::json!({"type":"response.output_text.delta","delta":"chunk"})));
    let mut flood = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
    let provider = self::provider(&flood);
    let mut stream = provider.stream_response(&context("same"));
    assert!(matches!(stream.next().await.unwrap().unwrap(), LlmResponse::Start { .. }));
    let first = flood.captured().await;
    drop(stream);
    assert_eq!(flood.closed().await, first.connection);
    collect(&provider, &context("same")).await;
    assert_ne!(flood.captured().await.connection, first.connection);
}

#[tokio::test]
async fn authentication_rejection_reloads_credentials_and_invalidates_session_identity() {
    use aether_auth::OAuthCredentialStorage;
    let mut events = text_events("one", "Hello");
    events.insert(1, serde_json::json!({"type":"response.metadata","headers":{"x-codex-turn-state":"old-token"}}));
    let rejected =
        serde_json::json!({"type":"error","status":401,"error":{"code":"invalid_api_key","message":"rejected"}});
    let mut server =
        FakeResponsesWebsocketServer::start(vec![Reply::events(events), Reply::events(vec![rejected])]).await;
    let store = Arc::new(FakeOAuthCredentialStore::new().with_credential("codex", credential("account-1")));
    let provider = CodexProvider::new(store.clone(), connection(&server.base_url), MODEL).unwrap();
    collect(&provider, &context("one")).await;
    let first = server.captured().await;
    let failed = provider.stream_response(&context("one")).collect::<Vec<_>>().await;
    assert_eq!(failed[0].as_ref().unwrap_err().provider().unwrap().kind, crate::ProviderErrorKind::Authentication);
    server.captured().await;
    store.save_credential("codex", credential("account-2")).await.unwrap();
    collect(&provider, &context("one")).await;
    let changed = server.captured().await;
    assert_ne!(changed.connection, first.connection);
    assert_eq!(changed.headers["chatgpt-account-id"], "account-2");
    assert!(changed.body["client_metadata"].get("x-codex-turn-state").is_none());
    assert!(changed.body.get("previous_response_id").is_none());
}
