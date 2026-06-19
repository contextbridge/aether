Auto-generated model catalog with metadata for all known LLM models.

The catalog is generated at build time by the `aether-llm-codegen` crate from
`models.json`. **Do not edit `generated.rs` by hand** -- modify `models.json` and rebuild instead.

# `LlmModel` enum

The central type is [`LlmModel`], with one variant per provider family:
`Anthropic(AnthropicModel)`, `OpenRouter(OpenRouterModel)`, `Gemini(GeminiModel)`,
`Ollama(String)`, `LlamaCpp(String)`, etc.

Key methods on `LlmModel`:

- [`all()`](LlmModel::all) -- All models in the catalog (static slice).
- [`provider()`](LlmModel::provider) -- Provider key (e.g. `"anthropic"`, `"openai"`).
- [`model_id()`](LlmModel::model_id) -- The model identifier sent to the API.
- [`context_window()`](LlmModel::context_window) -- Context window in tokens, if known.
- [`provider_display_name()`](LlmModel::provider_display_name) -- Human-readable provider name.
- [`required_env_var()`](LlmModel::required_env_var) -- The env var needed to use this model.

`LlmModel` implements `FromStr` and `Display` for parsing and formatting
`"provider:model_id"` strings (e.g. `"anthropic:claude-opus-4-6"`).

# Helper functions

- [`available_models()`] -- Returns catalog models whose required env var is set.
- [`get_local_models()`] -- Returns `available_models()` plus any locally discovered
  models (Ollama instances, llama.cpp servers).
