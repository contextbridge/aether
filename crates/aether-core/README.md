# Aether Core

Aether Core is a Rust library for building AI agents (LLM + prompt + tools, running in a loop). 

By default, agents have _no_ system prompt and _no_ tools — every token in the context window is yours to control. Tools come exclusively from [MCP](https://modelcontextprotocol.io/) servers, so you can extend agents in any language.

Agents run in dedicated [tokio tasks](https://tokio.rs) and communicate via async message passing. Hardware permitting, you can run hundreds of agents in a single process.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Installation](#installation)
- [Examples](#examples)
  - [Minimal Agent (No Tools)](#minimal-agent-no-tools)
  - [Agent with Tools and AGENTS.md system prompt](#agent-with-tools-and-agentsmd-system-prompt)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Installation

Add Aether to your `Cargo.toml`:

```toml
[dependencies]
aether-agent-core = "0.1"
```

## Examples

### Minimal Agent (No Tools)

```rust,no_run
use aether_core::core::{Command, Prompt, agent};
use aether_core::events::{AgentEvent, MessageEvent, ToolEvent, TurnEvent};
use llm::providers::openrouter::OpenRouterProvider;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Choose your LLM. Alternatively use AnthropicProvider, LlamaCppProvider..etc
    // For this example, OPENROUTER_API_KEY needs to be set in your environment
    let llm = OpenRouterProvider::default("z-ai/glm-4.6")?;

   // 2. Create an Agent
    let (tx, mut rx, _handle) = agent(llm) // <-- Give it an LLM
        .system_prompt(Prompt::text("You are a helpful assistant.")) // <-- Give it a system prompt
        .spawn() // <-- Spawn it into a tokio task
        .await?;

    // 3. Send the agent a message
    tx.send(Command::text("Explain async Rust in one paragraph"))
        .await?;

    // 4. Stream the agent's response back
    loop {
        match rx.recv().await {
            Some(AgentEvent::Message(MessageEvent::Text { chunk, is_complete, .. })) => {
                if !is_complete { print!("{chunk}"); io::stdout().flush()?; }
            }
            Some(AgentEvent::Turn(TurnEvent::Ended { outcome })) => {
                println!("Turn ended: {outcome:?}");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
```

### Agent with Tools and AGENTS.md system prompt

Create a `mcp.json` file in the current working directory:

```json
{
  "servers": {
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

And bring Mr. `BotBot` to life!

```rust,no_run
use aether_core::core::{Command, Prompt, agent};
use aether_core::events::{AgentEvent, MessageEvent, ToolEvent, TurnEvent};
use aether_core::mcp::mcp;
use llm::providers::openrouter::OpenRouterProvider;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = OpenRouterProvider::default("z-ai/glm-4.6")?;

    // 1. Connect to MCP servers
    let mut mcp_runtime = mcp(".")
        .from_json_files(&["mcp.json"]) // <-- Load MCP servers from one or more JSON files
        .await?
        .spawn() // <-- Spawn the MCP client into a tokio task (multiple agents can use it)
        .await?;
    let snapshot = mcp_runtime.block_until_ready().await.expect("MCP should initialize");

    // 2. Create Agent
    let (tx, mut rx, _handle) = agent(llm)
        .system_prompt(Prompt::file("AGENTS.md", ".")) // <-- Load system prompt from AGENTS.md
        .tools(mcp_runtime.command_tx, snapshot.tool_definitions) // <-- Give the agent MCP tools
        .spawn()
        .await?;

   // Send your agent a message and stream the results back
    tx.send(Command::text(
        "Read the README.md file and summarize it",
    ))
    .await?;

    loop {
        match rx.recv().await {
            Some(AgentEvent::Message(MessageEvent::Text { chunk, is_complete, .. })) => {
                if !is_complete {
                    print!("{chunk}");
                    io::stdout().flush().unwrap();
                } else {
                    println!();
                }
            }
            Some(AgentEvent::Tool(ToolEvent::Call { request, .. })) => {
                println!("\nCalling tool: {}", request.name);
            }
            Some(AgentEvent::Tool(ToolEvent::Result { result, .. })) => {
                println!("Tool '{}' completed", result.name);
            }
            Some(AgentEvent::Tool(ToolEvent::Error { error, .. })) => {
                eprintln!("Tool '{}' failed: {}", error.name, error.error);
            }
            Some(AgentEvent::Tool(ToolEvent::Progress { .. })) => {
                // Tool progress updates (can be used to show progress bars, etc.)
            }
            Some(AgentEvent::Turn(TurnEvent::Ended { outcome })) => {
                println!("\nTurn ended: {outcome:?}");
                break;
            }
            _ => {}
            None => break,
        }
    }

    Ok(())
}
```

## License

MIT
