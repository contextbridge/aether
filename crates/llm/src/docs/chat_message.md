A message in the conversation history.

Each variant represents a different role or message type in the conversation sent to the LLM provider.

# Variants

- **`System`** -- The system prompt. Typically the first message in the conversation.
- **`User`** -- User input, supporting multimodal content via `Vec<`[`ContentBlock`]`>`.
- **`Assistant`** -- The model's response. Includes text content, optional [`AssistantReasoning`], and any [`ToolCallRequest`]s the model made.
- **`ToolCallResult`** -- The result of executing a tool call, wrapping `Result<`[`ToolCallResult`](crate::ToolCallResult)`, `[`ToolCallError`]`>`.
- **`Error`** -- An error message recorded in conversation history.
- **`Summary`** -- A compacted summary replacing multiple earlier messages to reduce context usage. Created by [`Context::with_compacted_summary`](crate::Context::with_compacted_summary).

# Helper methods

- [`is_system`](ChatMessage::is_system), [`is_tool_result`](ChatMessage::is_tool_result), [`is_summary`](ChatMessage::is_summary) -- Variant checks.
- [`timestamp`](ChatMessage::timestamp) -- Returns the message timestamp (all variants except `ToolCallResult`).
- [`estimated_bytes`](ChatMessage::estimated_bytes) -- Rough byte-size estimate for pre-flight context checks.

# Serialization

Uses `#[serde(tag = "type", rename_all = "camelCase")]` -- each variant is serialized with a `"type"` field (e.g. `{"type": "system", "content": "..."}`) for use in JSON-based persistence and wire formats.
