use llm::LlmResponse;
use llm::providers::openai_compatible;
use llm::providers::openai_compatible::streaming::process_compatible_stream;
use llm::providers::openai_compatible::types::{
    ChatCompletionStreamChoice, ChatCompletionStreamResponse, ChatCompletionStreamResponseDelta, Usage,
};
use tokio_stream::StreamExt;

/// Test that negative token counts from `OpenRouter` are clamped to 0 by the
/// `Usage -> TokenUsage` conversion in `process_compatible_stream`.
#[tokio::test]
async fn test_openrouter_negative_token_handling() {
    let stream_items = vec![Ok::<ChatCompletionStreamResponse, std::io::Error>(ChatCompletionStreamResponse {
        id: "gen-123".to_string(),
        choices: vec![ChatCompletionStreamChoice {
            index: 0,
            delta: ChatCompletionStreamResponseDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: None,
            },
            finish_reason: Some(openai_compatible::types::FinishReason::Stop),
            logprobs: None,
        }],
        created: 1_234_567_890,
        model: "openai/gpt-3.5-turbo".to_string(),
        system_fingerprint: None,
        object: "chat.completion.chunk".to_string(),
        usage: Some(Usage {
            prompt_tokens: -5,
            completion_tokens: 10,
            total_tokens: 5,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }),
    })];

    let stream = tokio_stream::iter(stream_items);
    let mut processed_stream = Box::pin(process_compatible_stream(stream));

    let mut events = Vec::new();
    while let Some(event) = processed_stream.next().await {
        events.push(event.unwrap());
    }

    let usage = events
        .iter()
        .find_map(|e| match e {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        })
        .expect("usage event");

    assert_eq!(usage.input_tokens, 0, "Negative input tokens should be clamped to 0");
    assert_eq!(usage.output_tokens, 10, "Output tokens should be 10");
}

/// Verify usage data is captured when sent in a separate chunk after `finish_reason`,
/// matching `OpenRouter`'s actual behavior. See: <https://openrouter.ai/docs/guides/usage-accounting>.
#[tokio::test]
async fn test_openrouter_usage_in_separate_final_chunk() {
    let stream_items: Vec<Result<ChatCompletionStreamResponse, std::io::Error>> = vec![
        // Chunk 1: content
        Ok(ChatCompletionStreamResponse {
            id: "gen-123".to_string(),
            choices: vec![ChatCompletionStreamChoice {
                index: 0,
                delta: ChatCompletionStreamResponseDelta {
                    role: None,
                    content: Some("Hello world".to_string()),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: None,
                logprobs: None,
            }],
            created: 1_234_567_890,
            model: "openai/gpt-3.5-turbo".to_string(),
            system_fingerprint: None,
            object: "chat.completion.chunk".to_string(),
            usage: None,
        }),
        // Chunk 2: finish_reason but no usage
        Ok(ChatCompletionStreamResponse {
            id: "gen-123".to_string(),
            choices: vec![ChatCompletionStreamChoice {
                index: 0,
                delta: ChatCompletionStreamResponseDelta {
                    role: None,
                    content: None,
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: Some(openai_compatible::types::FinishReason::Stop),
                logprobs: None,
            }],
            created: 1_234_567_890,
            model: "openai/gpt-3.5-turbo".to_string(),
            system_fingerprint: None,
            object: "chat.completion.chunk".to_string(),
            usage: None,
        }),
        // Chunk 3: usage in separate final chunk with empty choices
        Ok(ChatCompletionStreamResponse {
            id: "gen-123".to_string(),
            choices: vec![],
            created: 1_234_567_890,
            model: "openai/gpt-3.5-turbo".to_string(),
            system_fingerprint: None,
            object: "chat.completion.chunk".to_string(),
            usage: Some(Usage {
                prompt_tokens: 15,
                completion_tokens: 25,
                total_tokens: 40,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }),
        }),
    ];

    let stream = tokio_stream::iter(stream_items);
    let mut processed_stream = Box::pin(process_compatible_stream(stream));

    let mut events = Vec::new();
    while let Some(event) = processed_stream.next().await {
        events.push(event.unwrap());
    }

    let usage = events
        .iter()
        .find_map(|e| match e {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        })
        .expect("usage event");
    assert_eq!(usage.input_tokens, 15);
    assert_eq!(usage.output_tokens, 25);

    let text_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            LlmResponse::Text { chunk } => Some(chunk.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text_events, vec!["Hello world".to_string()]);
}

/// Test that the `OpenRouterChatRequest` serializes the usage parameter correctly
#[test]
fn test_openrouter_request_serialization() {
    use llm::providers::openai_compatible::types::{CompatibleChatMessage, UserContent};
    use llm::providers::openrouter::{CacheControl, OpenRouterChatRequest, OpenRouterUsage};
    use serde_json;

    let request = OpenRouterChatRequest {
        model: "openai/gpt-3.5-turbo".to_string(),
        messages: vec![CompatibleChatMessage::User { content: UserContent::Text("Hello".to_string()) }],
        stream: Some(true),
        tools: None,
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_completion_tokens: None,
        stream_options: None,
        usage: Some(OpenRouterUsage { include: true }),
        presence_penalty: None,
        frequency_penalty: None,
        stop: None,
        response_format: None,
        reasoning_effort: None,
        cache_control: Some(CacheControl::ephemeral()),
    };

    let json = serde_json::to_value(&request).unwrap();

    // Verify the usage parameter is serialized correctly
    assert_eq!(
        json["usage"]["include"],
        serde_json::Value::Bool(true),
        "Usage parameter should be serialized with include: true"
    );
    assert_eq!(
        json["model"],
        serde_json::Value::String("openai/gpt-3.5-turbo".to_string()),
        "Model should be serialized correctly"
    );
}

/// Test that `cache_control` serializes as `{"type": "ephemeral"}` at the request root
#[test]
fn test_openrouter_cache_control_serialization() {
    use llm::providers::openrouter::CacheControl;

    let cache_control = CacheControl::ephemeral();
    let json = serde_json::to_value(&cache_control).unwrap();

    assert_eq!(json["type"], "ephemeral");
}

/// Test that `From<CompatibleChatRequest>` sets `cache_control` for prompt caching
#[test]
fn test_openrouter_from_compatible_request_includes_cache_control() {
    use llm::providers::openai_compatible::CompatibleChatRequest;
    use llm::providers::openai_compatible::types::{CompatibleChatMessage, UserContent};
    use llm::providers::openrouter::OpenRouterChatRequest;

    let compatible = CompatibleChatRequest {
        model: "zhipu/glm-5".to_string(),
        messages: vec![
            CompatibleChatMessage::System { content: "You are helpful.".to_string() },
            CompatibleChatMessage::User { content: UserContent::Text("Hello".to_string()) },
        ],
        stream: Some(true),
        tools: None,
        stream_options: None,
        reasoning_effort: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
    };

    let openrouter: OpenRouterChatRequest = compatible.into();

    let json = serde_json::to_value(&openrouter).unwrap();
    assert_eq!(
        json["cache_control"]["type"], "ephemeral",
        "From conversion should set cache_control for prompt caching"
    );
}
