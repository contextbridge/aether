Built-in LLM provider implementations.

Each submodule implements [`StreamingModelProvider`](crate::StreamingModelProvider) and [`ProviderFactory`](crate::ProviderFactory) for a specific LLM service.

# Available providers

| Module | Provider | Env var | Feature flag |
|--------|----------|---------|-------------|
| [`anthropic`] | Anthropic (Claude) | `ANTHROPIC_API_KEY` | -- |
| [`openai`] | `OpenAI` (GPT) | `OPENAI_API_KEY` | -- |
| [`openrouter`] | `OpenRouter` | `OPENROUTER_API_KEY` | -- |
| [`gemini`] | Google Gemini | `GEMINI_API_KEY` | -- |
| [`local::ollama`] | Ollama | -- (local) | -- |
| [`local::llama_cpp`] | llama.cpp | -- (local) | -- |
| [`openai_compatible`] | `DeepSeek`, ZAI, Moonshot | varies | -- |
| [`bedrock`] | AWS Bedrock | AWS credentials | `bedrock` |
| [`codex`] | `OpenAI` Codex (OAuth) | -- (OAuth) | `codex` |

# OpenAI-compatible providers

The [`openai_compatible`] module provides a shared [`GenericOpenAiProvider`](openai_compatible::generic::GenericOpenAiProvider) that works with any `OpenAI`-compatible API. `DeepSeek`, ZAI, and Moonshot use this with pre-configured [`ProviderConfig`](openai_compatible::generic::ProviderConfig) constants.

# Adding a new provider

1. Create a submodule under `providers/`.
2. Implement [`StreamingModelProvider`](crate::StreamingModelProvider) and [`ProviderFactory`](crate::ProviderFactory).
3. Register it in [`ModelProviderParser::default()`](crate::parser::ModelProviderParser::default).
4. Add model entries to `models.json` for the catalog.
