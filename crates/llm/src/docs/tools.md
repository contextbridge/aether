A tool that the LLM can invoke during generation.

Tools follow a request-response lifecycle:

1. **Define** -- Create [`ToolDefinition`]s with a name, description, and JSON Schema parameters, then pass them to the model via [`Context::set_tools`](crate::Context::set_tools).
2. **Request** -- The model emits a [`ToolCallRequest`] (streamed as `ToolRequestStart` -> `ToolRequestArg` -> `ToolRequestComplete` in [`LlmResponse`](crate::LlmResponse)).
3. **Execute** -- Run the requested tool and produce either a [`ToolCallResult`] (success) or a [`ToolCallError`] (failure).
4. **Return** -- Feed the result back to the model as a [`ChatMessage::ToolCallResult`](crate::ChatMessage::ToolCallResult) in the next turn.

# Related types

- [`ToolCallRequest`] -- What the model asks for: tool `id`, `name`, and `arguments` (JSON string).
- [`ToolCallResult`] -- Successful execution: includes the `result` string.
- [`ToolCallError`] -- Failed execution: includes the `error` string. Construct from a request with [`ToolCallError::from_request`].

The optional `server` field on `ToolDefinition` tracks which MCP server originally provided the tool, if any.

The optional `annotations` field preserves MCP tool hints such as `readOnlyHint`, `destructiveHint`, `idempotentHint`, and `openWorldHint`. These values are hints supplied by the server and are used by higher-level filtering policy; they are not a security boundary.
