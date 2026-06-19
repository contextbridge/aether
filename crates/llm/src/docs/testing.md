Test utilities for code that depends on [`StreamingModelProvider`](crate::StreamingModelProvider).

# `FakeLlmProvider`

A configurable fake that returns canned responses. Use it in place of real providers in unit tests:

```rust,no_run
use llm::testing::FakeLlmProvider;
use llm::{LlmResponse, Context};

// Returns "Hello!" then Done on the first call
let provider = FakeLlmProvider::with_single_response(vec![
    LlmResponse::text("Hello!"),
    LlmResponse::done(),
]);
```

**Multiple turns** -- Pass a `Vec<Vec<LlmResponse>>` to [`FakeLlmProvider::new`] where each inner vec is the response for one call. Calls beyond the provided responses return a bare `Done`.

**Context capture** -- Call [`captured_contexts()`](FakeLlmProvider::captured_contexts) to get an `Arc<Mutex<Vec<Context>>>` that records every context passed to `stream_response`.

**Customization** -- Chain [`with_display_name`](FakeLlmProvider::with_display_name) and [`with_context_window`](FakeLlmProvider::with_context_window) to configure the provider's metadata.

# `LlmResponseBuilder`

A builder for constructing response sequences with less boilerplate:

```rust,no_run
use llm::testing::llm_response;

let chunks = llm_response("msg-1")
    .text(&["Hello", " world"])
    .tool_call("tc-1", "read_file", &[r#"{"path":"#, r#""foo.rs"}"#])
    .build();
// Produces: Start -> Text("Hello") -> Text(" world") -> ToolRequestStart -> ... -> Done
```
