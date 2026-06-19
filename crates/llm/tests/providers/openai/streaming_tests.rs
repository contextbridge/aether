#![allow(deprecated)]

use async_openai::types::chat::{
    ChatChoiceStream, ChatCompletionMessageToolCallChunk, ChatCompletionStreamResponseDelta,
    CreateChatCompletionStreamResponse, FinishReason, FunctionCallStream,
};
use tokio_stream::StreamExt;

use llm::providers::openai::streaming::process_completion_stream;
use llm::{LlmResponse, StopReason};

fn chunk(
    tool_calls: Option<Vec<ChatCompletionMessageToolCallChunk>>,
    content: Option<&str>,
    finish_reason: Option<FinishReason>,
) -> CreateChatCompletionStreamResponse {
    CreateChatCompletionStreamResponse {
        choices: vec![ChatChoiceStream {
            delta: ChatCompletionStreamResponseDelta {
                content: content.map(String::from),
                tool_calls,
                role: None,
                refusal: None,
                #[allow(deprecated)]
                function_call: None,
            },
            finish_reason,
            index: 0,
            logprobs: None,
        }],
        id: "test".to_string(),
        created: 0,
        model: "test".to_string(),
        object: "chat.completion.chunk".to_string(),
        system_fingerprint: None,
        usage: None,
        service_tier: None,
    }
}

fn tool_start(index: u32, id: &str, name: &str) -> ChatCompletionMessageToolCallChunk {
    ChatCompletionMessageToolCallChunk {
        index,
        id: Some(id.to_string()),
        r#type: None,
        function: Some(FunctionCallStream { name: Some(name.to_string()), arguments: None }),
    }
}

fn tool_args(index: u32, args: &str) -> ChatCompletionMessageToolCallChunk {
    ChatCompletionMessageToolCallChunk {
        index,
        id: None,
        r#type: None,
        function: Some(FunctionCallStream { name: None, arguments: Some(args.to_string()) }),
    }
}

async fn collect_events(items: Vec<CreateChatCompletionStreamResponse>) -> Vec<LlmResponse> {
    let stream = tokio_stream::iter(items.into_iter().map(Ok::<_, std::io::Error>));
    let mut processed = Box::pin(process_completion_stream(stream));
    let mut events = Vec::new();
    while let Some(event) = processed.next().await {
        events.push(event.unwrap());
    }
    events
}

#[tokio::test]
async fn test_parallel_tool_calls() {
    let events = collect_events(vec![
        chunk(Some(vec![tool_start(0, "call_1", "function_a")]), None, None),
        chunk(Some(vec![tool_start(1, "call_2", "function_b")]), None, None),
        chunk(Some(vec![tool_args(0, r#"{"param":"#)]), None, None),
        chunk(Some(vec![tool_args(1, r#"{"value":"#)]), None, None),
        chunk(Some(vec![tool_args(0, r#""test"}"#)]), None, None),
        chunk(Some(vec![tool_args(1, "42}")]), None, None),
        chunk(None, None, Some(FinishReason::ToolCalls)),
    ])
    .await;

    assert!(matches!(events[0], LlmResponse::Start { .. }));

    let mut tool_starts = 0;
    let mut tool_args = 0;
    let mut tool_completions = 0;

    for event in &events {
        match event {
            LlmResponse::ToolRequestStart { id, name } => {
                tool_starts += 1;
                assert!(id == "call_1" || id == "call_2");
                assert!(name == "function_a" || name == "function_b");
            }
            LlmResponse::ToolRequestArg { id, chunk: _ } => {
                tool_args += 1;
                assert!(id == "call_1" || id == "call_2");
            }
            LlmResponse::ToolRequestComplete { tool_call } => {
                tool_completions += 1;
                if tool_call.id == "call_1" {
                    assert_eq!(tool_call.name, "function_a");
                    assert_eq!(tool_call.arguments, r#"{"param":"test"}"#);
                } else if tool_call.id == "call_2" {
                    assert_eq!(tool_call.name, "function_b");
                    assert_eq!(tool_call.arguments, r#"{"value":42}"#);
                } else {
                    panic!("Unexpected tool call id: {}", tool_call.id);
                }
            }
            _ => {}
        }
    }

    assert_eq!(tool_starts, 2, "Should have 2 tool request starts");
    assert_eq!(tool_args, 4, "Should have 4 tool argument chunks");
    assert_eq!(tool_completions, 2, "Should have 2 tool completions");

    assert!(matches!(events.last(), Some(LlmResponse::Done { stop_reason: Some(StopReason::ToolCalls) })));
}

#[tokio::test]
async fn test_tool_call_followed_by_content() {
    let events = collect_events(vec![
        chunk(Some(vec![tool_start(0, "call_1", "test_func")]), None, None),
        chunk(Some(vec![tool_args(0, "{}")]), None, None),
        chunk(None, Some("Here is the result"), None),
        chunk(None, None, Some(FinishReason::Stop)),
    ])
    .await;

    let mut tool_completion_index = None;
    let mut text_index = None;

    for (i, event) in events.iter().enumerate() {
        match event {
            LlmResponse::ToolRequestComplete { .. } => tool_completion_index = Some(i),
            LlmResponse::Text { .. } => text_index = Some(i),
            _ => {}
        }
    }

    assert!(tool_completion_index.is_some());
    assert!(text_index.is_some());
    assert!(tool_completion_index.unwrap() < text_index.unwrap(), "Tool completion should come before text content");
}
