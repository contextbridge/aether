Built-in LLM provider implementations.

Each provider implements [`StreamingModelProvider`](crate::StreamingModelProvider) for a specific LLM service.

# Available providers

| Module | Provider | Env var | Feature flag |
|--------|----------|---------|-------------|
| [`generic`] | Anthropic, `OpenAI`, Xiaomi `MiMo`, `DeepSeek`, Fireworks AI, Microsoft Foundry, Moonshot, ZAI, Google Gemini, `OpenRouter`, Ollama, llama.cpp | varies (none for local) | -- |
| [`bedrock`] | AWS Bedrock | AWS credentials | `bedrock` |
| [`codex`] | `OpenAI` Codex (OAuth) | -- (OAuth) | `codex` |

# Generic providers

The [`generic`] module provides a shared [`GenericProvider`](generic::GenericProvider) for endpoints that speak the `OpenAI` Chat Completions or Responses wire format, or the Anthropic Messages API. Anthropic, `OpenAI`, Xiaomi `MiMo`, `DeepSeek`, Fireworks AI, Microsoft Foundry, Moonshot, ZAI, Google Gemini, `OpenRouter`, and the local Ollama and llama.cpp servers are pre-configured [`ProviderConfig`](generic::ProviderConfig) constants; a new provider on any of these formats is one more constant. The [`local`] module discovers which models local servers have available.

# Adding a new provider

1. If it speaks one of the wire formats above, add a [`ProviderConfig`](generic::ProviderConfig) constant to `BUILT_INS` in [`generic`].
2. Otherwise, create a submodule under `providers/`, implement [`StreamingModelProvider`](crate::StreamingModelProvider), and register it in [`ModelProviderParser::default()`](crate::parser::ModelProviderParser::default).
3. Add model entries to `models.json` for the catalog.
