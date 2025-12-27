# Aether

Aether is a lightweight, "batteries-included" Rust library for building AI agents (LLMs + prompts + tools, running in a loop). It ships with support for multiple LLM providers, MCP servers, recursive loading of `AGENTS.md` files and more.

You can use Aether to build autonomous agents (ala Devin), or connect an agent to a UI to create something like Claude Code.

## Why Aether?

AI agents are simple: just a LLM + prompt + tool, running in a loop. Yet many frameworks over-abstract this into oblivion.

Aether aims to give you a great developer experience via a simple API that exposes a powerful set of composable primitives:

- **Agents**: Aether agents run in dedicated [tokio tasks](https://tokio.rs) and communicate via async message passing (i.e. they're [actors](https://en.wikipedia.org/wiki/Actor_model)). Hardware permitting, you can run hundreds of agents in a single process.

- **LLMs**: Aether supports models from Anthropic, OpenAI, OpenRouter, Llama.cpp and Ollama out of the box. You can implement your own provider via the `StreamableModelProvider` trait and combine multiple models from different providers into an "alloyed" model via `AlloyedModelProvider`.

- **Prompts**: Are just strings. But Aether provides nice helpers to do things like recursively load `AGENTS.md` files into your agent's system prompt and compose prompts from multiple sources.

- **Tools**: "MCP is all you need". Agents get tools _exclusively_  via MCP servers. You can easily configure your agent's MCP servers with a `mcp.json` file and run custom "in-memory" (Rust) MCP servers in dedicated tokio tasks.

- **Tests**: Aether provides a built-in set of test helpers that make it trivial to write robust unit and integration tests for your agents.

## Installation

Add Aether to your `Cargo.toml`:

```toml
[dependencies]
aether = "0.1"
```

## Examples

### Minimal Agent (No Tools)

```rust,no_run
use aether::{
    agent::{AgentMessage, UserMessage, agent},
    llm::openrouter::OpenRouterProvider,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Choose your LLM. Alternatively use AnthropicProvider, LlamaCppProvider..etc
    // For this example, OPENROUTER_API_KEY needs to be set in your environment
    let llm = OpenRouterProvider::default("z-ai/glm-4.6")?;

   // 2. Create an Agent
    let (tx, mut rx, _handle) = agent(llm) // <-- Give it an LLM
        .system("You are a helpful assistant.") // <-- Give it a system prompt
        .spawn() // <-- Spawn it into a tokio task
        .await?;

    // 3. Send the agent a message
    tx.send(UserMessage::text("Explain async Rust in one paragraph"))
        .await?;

    // 4. Stream the agent's response back
    loop {
        use AgentMessage::*;
        match rx.recv().await {
            Some(Text { chunk, is_complete, .. }) => {
                if !is_complete {
                    print!("{chunk}");
                    io::stdout().flush().unwrap();
                } else {
                    println!("\n");
                }
            }
            Some(ToolCall { .. }) => {
                // Tool calls not used in this minimal example
            }
            Some(ToolResult { .. }) => {
                // Tool results not used in this minimal example
            }
            Some(ToolError { .. }) => {
                // Tool errors not used in this minimal example
            }
            Some(ToolProgress { .. }) => {
                // Tool progress not used in this minimal example
            }
            Some(Done) => break,
            Some(Error { message }) => {
                eprintln!("Error: {message}");
                break;
            }
            Some(Cancelled { .. }) => {
                eprintln!("Agent cancelled");
                break;
            }
            Some(ContextCompacted { .. }) => {
                // Context was compacted to save space
            }
            None => break,
        }
    }

    Ok(())
}
```

### Agent with Tools and AGENTS.md system prompt

Create a `mcp.json` file in the current working directory:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed/directory"]
    },
    "playwright": {
      "command": "npx",
      "args": ["-y", "@executeautomation/playwright-mcp-server"]
    }
  }
}
```

And create an `AGENTS.md` file with a system prompt:

```markdown
# BotBot

You are Mr. BotBot, a kickass coding agent equipped with SOTA filesystem and web browsing tools...
```

And bring Mr. BotBot to life!

```rust,no_run
use aether::{
    agent::{AgentMessage, UserMessage, agent, Prompt},
    llm::openrouter::OpenRouterProvider,
    mcp::mcp,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = OpenRouterProvider::default("z-ai/glm-4.6")?;

    // 1. Connect to MCP servers
    let (tools, mcp_tx, _mcp_handle) = mcp()
        .from_json_file("mcp.json") // <-- Load MCP servers from JSON
        .await?
        .spawn() // <-- Spawn the MCP client into a tokio task (multiple agents can use it)
        .await?;

    // 2. Create Agent
    let (tx, mut rx, _handle) = agent(llm)
        .system(&Prompt::agents_md().build()?) // <-- Load system prompt from AGENTS.md (recursively searches parent directories)
        .tools(mcp_tx, tools) // <-- Give the agent MCP tools
        .spawn()
        .await?;

   // Send your agent a message and stream the results back
    tx.send(UserMessage::text(
        "Read the README.md file and summarize it",
    ))
    .await?;

    loop {
        use AgentMessage::*;
        match rx.recv().await {
            Some(Text { chunk, is_complete, .. }) => {
                if !is_complete {
                    print!("{chunk}");
                    io::stdout().flush().unwrap();
                } else {
                    println!();
                }
            }
            Some(ToolCall { request, .. }) => {
                println!("\nCalling tool: {}", request.name);
            }
            Some(ToolResult { result, .. }) => {
                println!("Tool '{}' completed", result.name);
            }
            Some(ToolError { error, .. }) => {
                eprintln!("Tool '{}' failed: {}", error.name, error.error);
            }
            Some(ToolProgress { .. }) => {
                // Tool progress updates (can be used to show progress bars, etc.)
            }
            Some(Done) => {
                println!("\nAgent finished");
                break;
            }
            Some(Error { message }) => {
                eprintln!("Error: {message}");
                break;
            }
            Some(Cancelled { .. }) => {
                eprintln!("Agent cancelled");
                break;
            }
            Some(ContextCompacted { .. }) => {
                // Context was compacted to save space
            }
            None => break,
        }
    }

    Ok(())
}
```

## License

MIT
