A streaming response event from an LLM provider.

Providers return an [`LlmResponseStream`](crate::LlmResponseStream) that yields a sequence of these events as the model generates its response. The stream follows a defined lifecycle:

```text
Start -> (Text | Reasoning | EncryptedReasoning | ToolRequest*)* -> Usage -> Done
```

# Text generation

- **`Start`** -- Stream opened, contains the `message_id`.
- **`Text`** -- A chunk of generated text.
- **`Reasoning`** -- A chunk of the model's chain-of-thought reasoning (visible summary).
- **`EncryptedReasoning`** -- Opaque encrypted reasoning content (model-specific, can be replayed to the same model in future turns).

# Tool calling

Tool calls are streamed in three phases:

1. **`ToolRequestStart`** -- The model begins a tool call (`id` + `name`).
2. **`ToolRequestArg`** -- Argument JSON arrives in chunks (same `id`).
3. **`ToolRequestComplete`** -- The fully assembled [`ToolCallRequest`] is ready to execute.

Multiple tool calls can be interleaved in a single response.

# Completion

- **`Usage`** -- Token usage statistics, carried as a [`TokenUsage`]. Required: `input_tokens`, `output_tokens`. Optional sub-categories (each filled in by the providers that expose it):
  - `cache_read_tokens` -- prompt tokens served from cache (`Anthropic`, `Bedrock`, `OpenAI` Chat/Responses/Codex, `OpenAI`-compat).
  - `cache_creation_tokens` -- prompt tokens written to cache on this turn (`Anthropic` `cache_creation_input_tokens`, `Bedrock` `cache_write_input_tokens`, `OpenRouter` `cache_write_tokens`).
  - `input_audio_tokens` -- input audio tokens, subset of `input_tokens` (`OpenAI` Chat, `OpenRouter`).
  - `input_video_tokens` -- input video tokens, subset of `input_tokens` (`OpenRouter`).
  - `reasoning_tokens` -- reasoning/thinking tokens, subset of `output_tokens` (`OpenAI` Chat/Responses/Codex, `OpenRouter`).
  - `output_audio_tokens` -- output audio tokens, subset of `output_tokens` (`OpenAI` Chat, `OpenRouter`).
  - `accepted_prediction_tokens` -- subset of `output_tokens` for predicted-output models (`OpenAI` Chat, `OpenRouter`).
  - `rejected_prediction_tokens` -- subset of `output_tokens` for predicted-output models (`OpenAI` Chat, `OpenRouter`).
- **`Done`** -- Stream complete, with an optional [`StopReason`].
- **`Error`** -- An error occurred during generation.

# Convenience constructors

Each variant has a corresponding constructor method (e.g. [`LlmResponse::text("chunk")`](LlmResponse::text), [`LlmResponse::done()`](LlmResponse::done)) to simplify test fixture construction.
