Parses `"provider:model"` strings into live [`StreamingModelProvider`] instances.

This is the primary entry point for creating providers from user-supplied model specifications at runtime. It handles provider lookup, credential loading, and model configuration in one step.

# Format

- **Single provider** -- `"anthropic:claude-sonnet-4-5-20250929"` or `"ollama:llama3.2"`
- **Multiple providers** -- `"anthropic:claude-sonnet-4-5-20250929,openai:gpt-4o"` creates an [`AlloyedModelProvider`](crate::alloyed::AlloyedModelProvider) that cycles between them.

# Built-in providers

[`Default::default()`](ModelProviderParser::default) registers all built-in providers: `anthropic`, `openai`, `openrouter`, `gemini`, `ollama`, `llamacpp`, `deepseek`, `moonshot`, `zai`, and (with feature flags) `bedrock` and `codex`.

# Custom providers

Register additional providers with [`with_provider`](ModelProviderParser::with_provider) (for types implementing [`ProviderFactory`]) or [`with_openai_provider`](ModelProviderParser::with_openai_provider) (for OpenAI-compatible APIs).

# Example

```rust,no_run
# async fn example() {
use llm::parser::ModelProviderParser;
use llm::StreamingModelProvider;

let parser = ModelProviderParser::default();
let (provider, model) = parser.parse("ollama:llama3.2").await.unwrap();
println!("Provider: {}", provider.display_name());
println!("Model: {model}");
# }
```
