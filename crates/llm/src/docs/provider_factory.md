Factory trait for constructing model providers from environment configuration.

This trait is separate from [`StreamingModelProvider`] because construction methods require `Sized`, which is incompatible with trait objects (`Box<dyn StreamingModelProvider>`). By splitting the factory methods into their own trait, the provider trait remains object-safe.

# Methods

- **`async from_env_with_connection(connection) -> Result<Self>`** -- Create a provider from environment variables (e.g. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) with [`ProviderConnectionConfig`](crate::ProviderConnectionConfig) overrides. Returns [`LlmError::MissingApiKey`](crate::LlmError::MissingApiKey) if the required variable is not set.

- **`async from_env() -> Result<Self>`** -- `from_env_with_connection` with the default connection.

- **`with_model(self, model: &str) -> Self`** -- Set or override the model for this provider. Returns `self` for builder-style chaining.

# Example

```rust,no_run
# async fn example() {
use llm::{ProviderFactory, StreamingModelProvider};
use llm::providers::anthropic::AnthropicProvider;

let provider = AnthropicProvider::from_env().await
    .expect("ANTHROPIC_API_KEY must be set")
    .with_model("claude-sonnet-4-5-20250929");

println!("Using: {}", provider.display_name());
# }
```
