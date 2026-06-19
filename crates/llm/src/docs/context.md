Manages the conversation state sent to an LLM provider.

A `Context` bundles together three things:
1. **Messages** -- the conversation history as a `Vec<`[`ChatMessage`]`>`
2. **Tools** -- the tools available to the model as a `Vec<`[`ToolDefinition`]`>`
3. **Configuration** -- optional [`ReasoningEffort`] and prompt cache key

Pass a `Context` to [`StreamingModelProvider::stream_response`](crate::StreamingModelProvider::stream_response) to generate a response.

# Construction

```rust,no_run
use llm::{Context, ChatMessage, ContentBlock, ToolDefinition, types::IsoString};

let context = Context::new(
    vec![
        ChatMessage::System {
            content: "You are a helpful assistant.".into(),
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

# Managing messages

- [`add_message`](Context::add_message) -- Append a single message.
- [`push_assistant_turn`](Context::push_assistant_turn) -- Append an assistant response together with its tool call results in one step.
- [`clear_conversation`](Context::clear_conversation) -- Remove all non-system messages.

# Conversation compaction

Long conversations can be compacted to stay within context limits:

- [`messages_for_summary`](Context::messages_for_summary) -- Get non-system messages to feed to a summarizer.
- [`with_compacted_summary`](Context::with_compacted_summary) -- Create a new `Context` where conversation messages are replaced by a [`ChatMessage::Summary`]. The system prompt, tools, and configuration are preserved.

# Token estimation

[`estimated_token_count`](Context::estimated_token_count) provides a rough pre-flight estimate using a ~4 bytes/token heuristic. This is intentionally approximate -- use it to detect obvious overflow before calling the provider, not for precise accounting.

# Encrypted reasoning

[`filter_encrypted_reasoning`](Context::filter_encrypted_reasoning) creates a copy where encrypted reasoning content is kept only for messages from a matching model. This is necessary because encrypted reasoning tokens are model-specific and cannot be replayed to a different model.
