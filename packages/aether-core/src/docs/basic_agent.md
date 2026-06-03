```rust
use aether_core::core::{Prompt, agent};
use llm::{ProviderFactory, providers::openai_compatible::generic::{GenericOpenAiProvider, ZAI}};

async fn example() -> Result<(), Box<dyn std::error::Error>> {
    let provider = GenericOpenAiProvider::from_env(&ZAI)?
        .with_model("glm-5.1");

    let (_user_tx, _agent_rx, _handle) = agent(provider)
        .system_prompt(Prompt::text("You are a helpful assistant."))
        .spawn()
        .await?;

    Ok(())
}
```
