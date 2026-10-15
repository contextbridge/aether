Built-in LLM provider implementations.

Each submodule implements [`StreamingModelProvider`](crate::StreamingModelProvider) and [`ProviderFactory`](crate::ProviderFactory) for a specific LLM service.

# Available providers

| Module | Provider | Env var | Feature flag |
|--------|----------|---------|-------------|
| [`anthropic`] | Anthropic (Claude) | `ANTHROPIC_API_KEY` | -- |
| [`generic`] | `OpenAI`, Xiaomi `MiMo`, `DeepSeek`, Fireworks AI, Microsoft Foundry, Moonshot, ZAI, Google Gemini, `OpenRouter`, Ollama, llama.cpp | varies (none for local) | -- |
| [`bedrock`] | AWS Bedrock | AWS credentials | `bedrock` |
| [`codex`] | `OpenAI` Codex (OAuth) | -- (OAuth) | `codex` |

# Generic providers

The [`generic`] module provides a shared [`GenericProvider`](generic::GenericProvider) for endpoints that speak either the `OpenAI` Chat Completions or Responses wire format. `OpenAI`, Xiaomi `MiMo`, `DeepSeek`, Fireworks AI, Microsoft Foundry, Moonshot, ZAI, Google Gemini, `OpenRouter`, and the local Ollama and llama.cpp servers are pre-configured [`ProviderConfig`](generic::ProviderConfig) constants; a new provider on either format is one more constant. The [`local`] module discovers which models local servers have available.

# Adding a new provider

1. Create a submodule under `providers/`.
2. Implement [`StreamingModelProvider`](crate::StreamingModelProvider) and [`ProviderFactory`](crate::ProviderFactory).
3. Register it in [`ModelProviderParser::default()`](crate::parser::ModelProviderParser::default).
4. Add model entries to `models.json` for the catalog.
