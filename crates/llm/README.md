# aether-llm

Multi-provider LLM abstraction layer for Rust. Write your code once, then swap between Anthropic, `OpenAI`, `OpenRouter`, Ollama, and more by changing a single string.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quick start](#quick-start)
- [Examples](#examples)
  - [Conversation with a system prompt](#conversation-with-a-system-prompt)
  - [Tool use](#tool-use)
  - [Switching providers](#switching-providers)
  - [Direct provider construction](#direct-provider-construction)
- [Providers](#providers)
- [Documentation](#documentation)
- [Key Types](#key-types)
- [Feature Flags](#feature-flags)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Quick start

Parse a `"provider:model"` string into a provider, build a context, and stream the response:

```rust,no_run
use llm::parser::ModelProviderParser;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StreamingModelProvider};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> llm::Result<()> {
    let parser = ModelProviderParser::default();
    let (provider, _model) = parser.parse("anthropic:claude-sonnet-4-5-20250929").await?;

    let context = Context::new(
        vec![ChatMessage::User {
            content: vec![ContentBlock::text("Explain ownership in Rust in two sentences.")],
            timestamp: IsoString::now(),
        }],
        vec![],
    );

    let mut stream = provider.stream_response(&context);
    while let Some(Ok(event)) = stream.next().await {
        if let LlmResponse::Text { chunk } = event {
            print!("{chunk}");
        }
    }
    Ok(())
}
```

## Examples

### Conversation with a system prompt

```rust,no_run
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context};

let context = Context::new(
    vec![
        ChatMessage::System {
            content: "You are a helpful assistant that responds in haiku.".into(),
            timestamp: IsoString::now(),
        },
        ChatMessage::User {
            content: vec![ContentBlock::text("What is Rust?")],
            timestamp: IsoString::now(),
        },
    ],
    vec![], // no tools
);
```

### Tool use

Define tools with JSON Schema, then feed results back after execution:

```rust,no_run
use llm::types::IsoString;
use llm::{
    AssistantReasoning, ChatMessage, ContentBlock, Context, ToolCallResult, ToolDefinition,
};

let tools = vec![ToolDefinition::new(
    "get_weather",
    "Get current weather for a city",
    r#"{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}"#,
)];

let mut context = Context::new(
    vec![ChatMessage::User {
        content: vec![ContentBlock::text("What's the weather in Tokyo?")],
        timestamp: IsoString::now(),
    }],
    tools,
);

// After streaming the response and executing the tool call,
// feed the result back into the context:
context.push_assistant_turn(
    "Let me check the weather.",
    AssistantReasoning::default(),
    vec![Ok(ToolCallResult {
        id: "call_1".into(),
        name: "get_weather".into(),
        arguments: r#"{"city":"Tokyo"}"#.into(),
        result: "72F, sunny".into(),
    })],
);
// Then call provider.stream_response(&context) again for the final answer.
```

### Switching providers

`ModelProviderParser` accepts any supported `"provider:model"` string, so switching is a one-line change:

```rust,no_run
# async fn example() {
use llm::parser::ModelProviderParser;

let parser = ModelProviderParser::default();

// Cloud providers (need API keys in env)
let (provider, _) = parser.parse("anthropic:claude-sonnet-4-5-20250929").await.unwrap();
let (provider, _) = parser.parse("openai:gpt-4o").await.unwrap();
let (provider, _) = parser.parse("openrouter:moonshotai/kimi-k2").await.unwrap();

// Local models (no API key needed)
let (provider, _) = parser.parse("ollama:llama3.2").await.unwrap();
let (provider, _) = parser.parse("llamacpp").await.unwrap();
# }
```

### Direct provider construction

When you need fine-grained control (temperature, max tokens), construct the provider directly:

```rust,no_run
# async fn example() {
use llm::providers::anthropic::AnthropicProvider;
use llm::ProviderFactory;

let provider = AnthropicProvider::from_env().await
    .unwrap()
    .with_model("claude-sonnet-4-5-20250929")
    .with_temperature(0.7)
    .with_max_tokens(4096);
# }
```

## Providers

| Provider | Example model string | Env var |
|----------|---------------------|---------|
| Anthropic | `anthropic:claude-sonnet-4-5-20250929` | `ANTHROPIC_API_KEY` |
| `OpenAI` | `openai:gpt-4o` | `OPENAI_API_KEY` |
| `OpenRouter` | `openrouter:moonshotai/kimi-k2` | `OPENROUTER_API_KEY` |
| ZAI | `zai:GLM-4.6` | `ZAI_API_KEY` |
| AWS Bedrock | `bedrock:us.anthropic.claude-sonnet-4-5-20250929-v1:0` | AWS credentials |
| Ollama | `ollama:llama3.2` | None (local) |
| Llama.cpp | `llamacpp` | None (local) |

## Documentation

Full API documentation is available on [docs.rs](https://docs.rs/aether-llm).

Key entry points:
- [`StreamingModelProvider`] -- the core trait all providers implement
- [`Context`] -- conversation state management
- [`ChatMessage`] -- message types for building conversations
- [`LlmResponse`] -- streaming response events
- [`ModelProviderParser`](parser::ModelProviderParser) -- parse `"provider:model"` strings into providers

## Key Types

- **`StreamingModelProvider`** -- Core trait for all LLM providers. Implement this to add a new provider.
- **`Context`** -- Manages the message history, tool definitions, and reasoning effort sent to the model.
- **`ChatMessage`** -- Message enum with variants for user, assistant, and tool call messages.
- **`ToolDefinition`** -- Describes a tool the model can invoke (name, description, JSON schema).
- **`LlmModel`** -- Catalog of known models with metadata (context window, capabilities).

## Feature Flags

| Feature | Description |
|---------|-------------|
| `bedrock` | AWS Bedrock provider support |
| `oauth` | OAuth authentication (used by Codex provider) |
| `codex` | `OpenAI` Codex provider (implies `oauth`) |

## License

MIT
