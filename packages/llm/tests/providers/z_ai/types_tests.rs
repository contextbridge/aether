use async_openai::types::chat::Role;
use llm::providers::openai_compatible::ChatCompletionStreamResponse;

/// Test that we can deserialize the actual Z.ai response format
/// The error showed this JSON structure:
/// {"id":"202510181239034578873d49744455","created":1760762344,"model":"glm-4.6","choices":[{"index":0,"delta":{"role":"assistant","content":"\n"}}]}

#[test]
fn test_deserialize_zai_response_missing_object_field() {
    // This is the actual format Z.ai returns - missing the "object" field
    let json = r#"{
        "id": "202510181239034578873d49744455",
        "created": 1760762344,
        "model": "glm-4.6",
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": "\n"
            }
        }]
    }"#;

    // This should not panic and should deserialize successfully
    let result: Result<ChatCompletionStreamResponse, _> = serde_json::from_str(json);

    assert!(result.is_ok(), "Failed to deserialize Z.ai response: {:?}", result.err());

    let response = result.unwrap();
    assert_eq!(response.id, "202510181239034578873d49744455");
    assert_eq!(response.model, "glm-4.6");
    assert_eq!(response.created, 1_760_762_344);
    // Should have default object value
    assert_eq!(response.object, "chat.completion.chunk");
    assert_eq!(response.choices.len(), 1);

    let choice = &response.choices[0];
    assert_eq!(choice.index, 0);
    assert_eq!(choice.delta.role, Some(Role::Assistant));
    assert_eq!(choice.delta.content, Some("\n".to_string()));
}

#[test]
fn test_deserialize_zai_response_with_finish_reason() {
    let json = r#"{
        "id": "test123",
        "created": 1760762344,
        "model": "glm-4.6",
        "choices": [{
            "index": 0,
            "delta": {
                "content": "Hello world"
            },
            "finish_reason": "stop"
        }]
    }"#;

    let result: Result<ChatCompletionStreamResponse, _> = serde_json::from_str(json);

    assert!(result.is_ok());
    let response = result.unwrap();

    assert_eq!(response.choices[0].delta.content, Some("Hello world".to_string()));
    assert!(response.choices[0].finish_reason.is_some());
}

#[test]
fn test_deserialize_zai_response_with_tool_calls() {
    let json = r#"{
        "id": "test123",
        "created": 1760762344,
        "model": "glm-4.6",
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\":"
                    }
                }]
            }
        }]
    }"#;

    let result: Result<ChatCompletionStreamResponse, _> = serde_json::from_str(json);

    assert!(result.is_ok(), "Failed to deserialize: {:?}", result.err());
    let response = result.unwrap();

    let choice = &response.choices[0];
    assert!(choice.delta.tool_calls.is_some());

    let tool_calls = choice.delta.tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);

    let tool_call = &tool_calls[0];
    assert_eq!(tool_call.index, 0);
    assert_eq!(tool_call.id, Some("call_abc123".to_string()));
    assert_eq!(tool_call.tool_type, Some("function".to_string()));

    let function = tool_call.function.as_ref().unwrap();
    assert_eq!(function.name, Some("get_weather".to_string()));
    assert_eq!(function.arguments, Some("{\"location\":".to_string()));
}

#[test]
fn test_deserialize_zai_network_error_finish_reason() {
    let json = r#"{
        "id": "202603151451299c7a89c25180405d",
        "created": 1773557520,
        "model": "glm-5",
        "choices": [{
            "index": 0,
            "finish_reason": "network_error",
            "delta": {
                "role": "assistant",
                "content": ""
            }
        }]
    }"#;

    let result: Result<ChatCompletionStreamResponse, _> = serde_json::from_str(json);
    assert!(result.is_ok(), "Failed to deserialize Z.ai network_error response: {:?}", result.err());

    let response = result.unwrap();
    assert!(response.choices[0].finish_reason.is_some());
}

#[test]
fn test_zai_network_error_yields_retryable_server_error() {
    use llm::LlmError;
    use llm::providers::openai_compatible::process_compatible_stream;

    let json = r#"{
        "id": "202603151451299c7a89c25180405d",
        "created": 1773557520,
        "model": "glm-5",
        "choices": [{
            "index": 0,
            "finish_reason": "network_error",
            "delta": {
                "role": "assistant",
                "content": ""
            }
        }]
    }"#;

    let response: ChatCompletionStreamResponse = serde_json::from_str(json).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        use tokio_stream::StreamExt;

        let stream = tokio_stream::iter(vec![Ok::<_, std::io::Error>(response)]);
        let mut processed = Box::pin(process_compatible_stream(stream));

        let mut events = Vec::new();
        while let Some(event) = processed.next().await {
            events.push(event);
        }

        let last = events.last().expect("stream must yield at least one event");
        let err = last.as_ref().expect_err("network_error finish reason must surface as Err");
        assert!(matches!(err, LlmError::ServerError { status: None, .. }), "got {err:?}");
        assert!(err.is_retryable(), "network_error must be retryable so the agent recovers without user input");
    });
}
