The core abstraction for all LLM providers. Every provider in this crate implements this trait, and all consumer code should depend on it rather than on concrete types.

The trait follows a streaming design: [`stream_response`](StreamingModelProvider::stream_response) returns an [`LlmResponseStream`] that yields [`LlmResponse`] events as the model generates output. This allows callers to display tokens incrementally rather than waiting for the full response.

# Methods

- **`stream_response(&self, context: &Context) -> LlmResponseStream`** -- Send the conversation [`Context`] to the model and receive a stream of response events. The stream follows the lifecycle: `Start` -> `Text`/`Reasoning`/`ToolRequest*` -> `Usage` -> `Done`.

- **`model(&self) -> Option<LlmModel>`** -- The [`LlmModel`] this provider is configured to use. Returns `None` for test fakes or when the model identity is unknown at compile time.

- **`display_name(&self) -> String`** -- A human-readable provider and model (e.g. `"Anthropic (claude-sonnet-4-5)"`). Used in UI and logging. Defaults to the [`model`](StreamingModelProvider::model)'s provider display name and ID.

- **`context_window(&self) -> Option<u32>`** -- The model's context window size in tokens. Defaults to the [`model`](StreamingModelProvider::model)'s catalog entry; `None` where the window is unknown (e.g. Ollama, llama.cpp).

# Blanket implementations

The trait is implemented for `Box<dyn StreamingModelProvider>` and `Arc<T>` where `T: StreamingModelProvider`, so providers can be used behind trait objects or shared across threads without additional wrapping.

# Usage

```rust,no_run
use llm::{StreamingModelProvider, Context, ChatMessage, ContentBlock, LlmResponse};
use tokio_stream::StreamExt;

async fn ask(provider: &dyn StreamingModelProvider) {
    let context = Context::new(
        vec![ChatMessage::User {
            message_id: llm::MessageId::new(),
            content: vec![ContentBlock::text("Hello!")],
            timestamp: llm::types::IsoString::now(),
        }],
        vec![],
    );
    let mut stream = provider.stream_response(&context);
    while let Some(Ok(event)) = stream.next().await {
        if let LlmResponse::Text { chunk } = event {
            print!("{chunk}");
        }
    }
}
```

# See also

- [`ProviderFactory`] -- Construction trait, separated for dyn-compatibility.
- [`Context`] -- The conversation state passed to `stream_response`.
- [`LlmResponse`] -- The events yielded by the response stream.
- [`AlloyedModelProvider`](crate::alloyed::AlloyedModelProvider) -- Round-robin wrapper over multiple providers.
