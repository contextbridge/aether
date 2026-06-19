A [`StreamingModelProvider`] that cycles through multiple providers in round-robin order.

Each call to [`stream_response`](StreamingModelProvider::stream_response) advances to the next provider in the list. This allows alternating between models with different strengths on successive turns.

# Behavior

- **`stream_response`** -- Uses the current provider, then advances the index.
- **`display_name`** -- Returns the *current* provider's name (does not advance).
- **`context_window`** -- Returns the *minimum* context window across all providers.
  Returns `None` if any provider's window is unknown.
- **`model`** -- Returns the current provider's model identity.

# Example

```rust,no_run
use llm::alloyed::AlloyedModelProvider;
use llm::StreamingModelProvider;

fn create_alloyed(providers: Vec<Box<dyn StreamingModelProvider>>) -> AlloyedModelProvider {
    AlloyedModelProvider::new(providers)
    // First stream_response() uses providers[0],
    // second uses providers[1], etc.
}
```

Created automatically by [`ModelProviderParser::parse`](crate::parser::ModelProviderParser::parse)
when given a comma-separated model string like `"anthropic:claude-sonnet-4-5-20250929,openai:gpt-4o"`.
