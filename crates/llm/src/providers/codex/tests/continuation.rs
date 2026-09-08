use super::test_server::{FakeResponsesWebsocketServer, Reply, text_events};
use super::{collect, context, full_history, provider};
use crate::{ChatMessage, LlmResponse, StreamingModelProvider};
use futures::StreamExt;
use tokio::sync::oneshot;

#[tokio::test]
async fn three_requests_chain_from_full_history_not_previous_delta() {
    let mut server = FakeResponsesWebsocketServer::start(vec![
        Reply::events(text_events("one", "Hello")),
        Reply::events(text_events("two", "Again")),
        Reply::events(text_events("three", "Bye")),
    ])
    .await;
    let provider = provider(&server);
    let mut context = context("same");
    for (index, text) in ["Hello", "Again", "Bye"].into_iter().enumerate() {
        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let request = server.captured().await;
        assert_eq!(request.effective_input, full_history(&context));
        if index > 0 {
            assert_eq!(request.body["input"].as_array().unwrap().len(), 1);
            assert_eq!(request.body["previous_response_id"], if index == 1 { "one" } else { "two" });
        }
        context.push_assistant_turn(text, crate::AssistantReasoning::default(), vec![]);
        context.add_message(ChatMessage::user("next"));
    }
}

#[tokio::test]
async fn tool_results_chain_with_call_id_and_exact_arguments_and_reasoning() {
    let events =
        serde_json::from_str(include_str!("../../../../tests/fixtures/codex_websocket/tool_reasoning.json")).unwrap();
    let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
    let provider = provider(&server);
    let mut context = context("same");
    let responses = collect(&provider, &context).await;
    let call = responses
        .iter()
        .find_map(|response| match response {
            LlmResponse::ToolRequestComplete { tool_call } => Some(tool_call.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(call.id, "call_1");
    assert_eq!(call.arguments, "{ \"path\" : \"file\" }");
    assert!(
        responses.iter().any(|response| matches!(response, LlmResponse::Reasoning { chunk } if chunk == "Checking"))
    );
    assert!(responses.iter().any(
        |response| matches!(response, LlmResponse::Usage { tokens } if tokens.reasoning_tokens.unwrap().get() == 3)
    ));
    let encrypted = responses.iter().find_map(|response| match response {
        LlmResponse::EncryptedReasoning { id, content } => Some(crate::EncryptedReasoningContent {
            id: id.clone(),
            content: content.clone(),
            model: provider.model().unwrap(),
        }),
        _ => None,
    });
    context.push_assistant_turn(
        "",
        crate::AssistantReasoning { summary_text: Some("Checking".into()), encrypted_content: encrypted },
        vec![Ok(crate::ToolCallResult {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
            result: "file contents".into(),
        })],
    );
    collect(&provider, &context).await;
    let first = server.captured().await;
    let second = server.captured().await;
    assert_eq!(first.connection, second.connection);
    assert_eq!(second.body["previous_response_id"], "tool-response");
    assert_eq!(second.body["input"].as_array().unwrap().len(), 1);
    assert_eq!(second.body["input"][0]["call_id"], "call_1");
    assert_eq!(second.effective_input, full_history(&context));
    assert_eq!(
        second.effective_input[1],
        serde_json::json!({
            "type":"reasoning", "id":"reasoning-1", "encrypted_content":"opaque-content", "summary":[]
        })
    );
    assert_eq!(
        second.effective_input[2],
        serde_json::json!({
            "type":"function_call", "call_id":"call_1", "name":"read", "arguments":"{ \"path\" : \"file\" }"
        })
    );
    assert_eq!(
        second.effective_input[3],
        serde_json::json!({
            "type":"function_call_output", "call_id":"call_1", "output":"file contents"
        })
    );
}

#[tokio::test]
async fn reasoning_then_text_chains_with_history_matching_full_replay() {
    let events =
        serde_json::from_str(include_str!("../../../../tests/fixtures/codex_websocket/reasoning_text.json")).unwrap();
    let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
    let provider = provider(&server);
    let mut context = context("same");
    let responses = collect(&provider, &context).await;
    let encrypted = responses.iter().find_map(|response| match response {
        LlmResponse::EncryptedReasoning { id, content } => Some(crate::EncryptedReasoningContent {
            id: id.clone(),
            content: content.clone(),
            model: provider.model().unwrap(),
        }),
        _ => None,
    });
    context.push_assistant_turn(
        "Hello",
        crate::AssistantReasoning { summary_text: None, encrypted_content: encrypted },
        vec![],
    );
    context.add_message(ChatMessage::user("next"));
    collect(&provider, &context).await;
    server.captured().await;
    let second = server.captured().await;
    assert_eq!(second.body["previous_response_id"], "mixed-response");
    assert_eq!(second.body["input"].as_array().unwrap().len(), 1);
    assert_eq!(second.effective_input, full_history(&context));
}

#[tokio::test]
async fn reordered_parallel_tool_calls_reset_continuation() {
    for reverse in [false, true] {
        let mut events = vec![serde_json::json!({"type":"response.created","response":{"id":"parallel"}})];
        let mut output = Vec::new();
        for index in 0..2 {
            let item = serde_json::json!({"type":"function_call","id":format!("item-{index}"),"call_id":format!("call-{index}"),"name":"read","arguments":"{}","status":"completed"});
            let mut started = item.clone();
            started["arguments"] = "".into();
            started["status"] = "in_progress".into();
            events.push(serde_json::json!({"type":"response.output_item.added","output_index":index,"item":started}));
            events.push(
                serde_json::json!({"type":"response.function_call_arguments.delta","output_index":index,"delta":"{}"}),
            );
            events.push(serde_json::json!({"type":"response.function_call_arguments.done","output_index":index}));
            output.push(item);
        }
        events.push(serde_json::json!({"type":"response.completed","response":{"id":"parallel","status":"completed","output":output}}));
        let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
        let provider = provider(&server);
        let mut context = context("same");
        let responses = collect(&provider, &context).await;
        let mut results = responses
            .into_iter()
            .filter_map(|response| match response {
                LlmResponse::ToolRequestComplete { tool_call } => Some(Ok(crate::ToolCallResult {
                    id: tool_call.id,
                    name: tool_call.name,
                    arguments: tool_call.arguments,
                    result: "contents".into(),
                })),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        if reverse {
            results.reverse();
        }
        context.push_assistant_turn("", crate::AssistantReasoning::default(), results);
        collect(&provider, &context).await;
        let first = server.captured().await;
        let second = server.captured().await;
        assert_eq!(first.connection, second.connection);
        assert_eq!(second.body.get("previous_response_id").is_none(), reverse);
    }
}

#[tokio::test]
async fn continuation_preserves_wire_settings_and_effective_effort() {
    for (first_effort, next_effort, continues) in [
        (Some(crate::ReasoningEffort::Max), Some(crate::ReasoningEffort::Max), true),
        (Some(crate::ReasoningEffort::Max), Some(crate::ReasoningEffort::High), false),
        (Some(crate::ReasoningEffort::High), Some(crate::ReasoningEffort::Max), false),
        (None, Some(crate::ReasoningEffort::Medium), true),
    ] {
        let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(text_events("one", "Hello"))]).await;
        let provider = provider(&server);
        let mut context = context("same");
        context.set_reasoning_effort(first_effort);
        context.set_system_content("Instructions".into());
        context.set_prompt_cache_key(Some("cache".into()));
        context.set_tools(vec![crate::ToolDefinition::new("read", "Read", serde_json::json!({"type":"object"}))]);
        collect(&provider, &context).await;
        context.push_assistant_turn("Hello", crate::AssistantReasoning::default(), vec![]);
        context.add_message(ChatMessage::user("next"));
        context.set_reasoning_effort(next_effort);
        collect(&provider, &context).await;
        let first = server.captured().await;
        let second = server.captured().await;
        assert_eq!(second.body.get("previous_response_id").is_some(), continues);
        assert_eq!(second.body["reasoning"]["effort"], next_effort.unwrap().as_str());
        assert_eq!(second.effective_input, full_history(&context));
        for (key, value) in first.body.as_object().unwrap() {
            if !matches!(key.as_str(), "input" | "reasoning" | "client_metadata") {
                assert_eq!(&second.body[key], value, "{key}");
            }
        }
    }
}

#[tokio::test]
async fn history_and_settings_changes_disable_continuation() {
    for change in ["repeat", "edit", "compact", "tools", "effort", "instructions", "cache-key"] {
        let mut server = FakeResponsesWebsocketServer::start(vec![]).await;
        let provider = provider(&server);
        let mut context = context("same");
        collect(&provider, &context).await;
        if change != "repeat" {
            context.push_assistant_turn("Hello", crate::AssistantReasoning::default(), vec![]);
            context.add_message(ChatMessage::user("next"));
        }
        match change {
            "edit" => context.replace_conversation(vec![ChatMessage::user("edited")]),
            "compact" => context = context.with_compacted_summary("summary"),
            "tools" => context.set_tools(vec![crate::ToolDefinition::new(
                "read",
                "read",
                serde_json::json!({"type":"object"}),
            )]),
            "effort" => context.set_reasoning_effort(Some(crate::ReasoningEffort::High)),
            "instructions" => context.set_system_content("new instruction".into()),
            "cache-key" => context.set_prompt_cache_key(Some("different".into())),
            _ => {}
        }
        collect(&provider, &context).await;
        let first = server.captured().await;
        let second = server.captured().await;
        assert_eq!(first.connection, second.connection);
        assert!(second.body.get("previous_response_id").is_none(), "{change}");
    }
}

#[tokio::test]
async fn mismatched_missing_unsupported_or_malformed_output_sends_full_context() {
    for variant in [
        "mismatch",
        "missing",
        "wrong-id",
        "missing-id",
        "empty-id",
        "missing-status",
        "phase",
        "namespace",
        "unsupported",
        "future",
        "refusal",
        "malformed",
    ] {
        let mut events = text_events("one", "Hello");
        let response = &mut events[2]["response"];
        match variant {
            "mismatch" => response["output"][0]["content"][0]["text"] = "different".into(),
            "missing" => {
                response.as_object_mut().unwrap().remove("output");
            }
            "wrong-id" => response["id"] = "other".into(),
            "missing-id" => {
                response.as_object_mut().unwrap().remove("id");
            }
            "empty-id" => response["id"] = "".into(),
            "missing-status" => {
                response.as_object_mut().unwrap().remove("status");
            }
            "phase" => response["output"][0]["phase"] = "commentary".into(),
            "namespace" => response["output"][0]["namespace"] = "tools".into(),
            "unsupported" => response["output"] = serde_json::json!([{"type":"web_search_call"}]),
            "future" => response["output"] = serde_json::json!([{"type":"future_output"}]),
            "refusal" => response["output"][0]["content"][0] = serde_json::json!({"type":"refusal","refusal":"no"}),
            "malformed" => {
                response["output"] = serde_json::json!([{"type":"function_call","name":"read","arguments":"{}"}]);
            }
            _ => unreachable!(),
        }
        let mut server = FakeResponsesWebsocketServer::start(vec![Reply::events(events)]).await;
        let provider = provider(&server);
        let mut context = context("same");
        collect(&provider, &context).await;
        context.push_assistant_turn("Hello", crate::AssistantReasoning::default(), vec![]);
        context.add_message(ChatMessage::user("next"));
        collect(&provider, &context).await;
        server.captured().await;
        assert!(server.captured().await.body.get("previous_response_id").is_none(), "{variant}");
    }
}

#[tokio::test]
async fn queued_request_uses_checkpoint_from_completed_request() {
    let (release, wait) = oneshot::channel();
    let mut reply = Reply::events(text_events("one", "Hello"));
    reply.release = Some(wait);
    let mut server = FakeResponsesWebsocketServer::start(vec![reply]).await;
    let provider = provider(&server);
    let mut context = context("same");
    let first = tokio::spawn(provider.stream_response(&context).collect::<Vec<_>>());
    let first_request = server.captured().await;
    context.push_assistant_turn("Hello", crate::AssistantReasoning::default(), vec![]);
    context.add_message(ChatMessage::user("next"));
    let mut queued = provider.clone().stream_response(&context);
    assert!(futures::poll!(queued.next()).is_pending());
    release.send(()).unwrap();
    assert!(first.await.unwrap().iter().all(Result::is_ok));
    assert!(queued.collect::<Vec<_>>().await.iter().all(Result::is_ok));
    let continued = server.captured().await;
    assert_eq!(continued.connection, first_request.connection);
    assert_eq!(continued.body["previous_response_id"], "one");
    assert_eq!(continued.body["input"].as_array().unwrap().len(), 1);
    assert_eq!(continued.effective_input, full_history(&context));
}

#[tokio::test]
async fn pre_creation_recovery_reopens_once_with_full_context() {
    let rejection = serde_json::json!({"type":"error","status":400,"error":{"code":"previous_response_not_found","message":"expired"}});
    let mut server = FakeResponsesWebsocketServer::start(vec![
        Reply::events(text_events("one", "Hello")),
        Reply::events(vec![rejection]),
        Reply::events(text_events("two", "Recovered")),
    ])
    .await;
    let provider = provider(&server);
    let mut context = context("same");
    collect(&provider, &context).await;
    context.push_assistant_turn("Hello", crate::AssistantReasoning::default(), vec![]);
    context.add_message(ChatMessage::user("next"));
    let responses = collect(&provider, &context).await;
    assert_eq!(responses.iter().filter(|response| matches!(response, LlmResponse::Start { .. })).count(), 1);
    let first = server.captured().await;
    let rejected = server.captured().await;
    let recovered = server.captured().await;
    assert_eq!(first.connection, rejected.connection);
    assert_ne!(rejected.connection, recovered.connection);
    assert!(recovered.body.get("previous_response_id").is_none());
    assert_eq!(recovered.body["input"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn recovery_is_not_repeated_or_attempted_after_creation() {
    for created in [false, true] {
        let rejection = serde_json::json!({"type":"error","status":400,"error":{"code":"websocket_connection_limit_reached","message":"limit"}});
        let first = if created {
            vec![
                serde_json::json!({"type":"response.created","response":{"id":"one"}}),
                serde_json::json!({"type":"response.output_text.delta","delta":"partial"}),
                rejection.clone(),
            ]
        } else {
            vec![rejection.clone()]
        };
        let mut server =
            FakeResponsesWebsocketServer::start(vec![Reply::events(first), Reply::events(vec![rejection])]).await;
        let provider = provider(&server);
        let responses = provider.stream_response(&context("same")).collect::<Vec<_>>().await;
        let error = responses.last().unwrap().as_ref().unwrap_err().provider().unwrap();
        assert_eq!(error.kind, crate::ProviderErrorKind::StreamInterrupted);
        assert_eq!(error.code.as_deref(), Some("websocket_connection_limit_reached"));
        assert_eq!(responses.len(), if created { 3 } else { 1 });
        let first = server.captured().await;
        if !created {
            assert_ne!(first.connection, server.captured().await.connection);
        }
    }
}
