The reason an LLM stopped generating.

Returned inside [`LlmResponse::Done`] at the end of a response stream.

# Variants

- **`EndTurn`** -- The model finished its response naturally.
- **`Length`** -- The response was truncated because it hit the maximum output token limit.
- **`ToolCalls`** -- The model stopped to request one or more tool calls. The caller should execute the requested tools and send the results back in a new turn.
- **`ContentFilter`** -- The response was stopped by the provider's content filter.
- **`FunctionCall`** -- Legacy variant for older `OpenAI` function calling (deprecated in favor of `ToolCalls`).
- **`Unknown(String)`** -- An unrecognized stop reason from the provider.
