# Aether

**[Website](https://aether-agent.io)** | **[Quickstart Docs](https://aether-agent.io/getting-started/overview/#getting-started)**

[![CI](https://github.com/contextbridge/aether/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/contextbridge/aether/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/crates/v/aether-agent-cli.svg)](https://crates.io/crates/aether-agent-cli)
[![Rust](https://img.shields.io/badge/Made_with-Rust-orange.svg)](https://www.rust-lang.org)

Aether is an AI coding agent harness, written in Rust, that gives _you_ control over every token in context. Aether works with both local and remote LLM providers and runs in your terminal, IDE/Editor, or headlessly.

![Aether demo](demo.gif)

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Why Aether?](#why-aether)
- [Quick Start](#quick-start)
  - [1. **Install**](#1-install)
  - [2. **Create your first agent**](#2-create-your-first-agent)
  - [3. **Run it**](#3-run-it)
- [Using Aether as a library](#using-aether-as-a-library)
- [Packages](#packages)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->



## Why Aether?

Many harnesses ship with hardcoded system prompts and tools that bloat context and cause you to fight the harness for control. 

Aether takes a different approach. Agents begin as blank slates with _no_ system prompt and 0 tools, so every token is _yours_ to mold. Then, you add what you need. Aether is highly modular by design. You can go from a minimal agent to fully batteries-included with just a few lines of config.

- **Your Prompt**: System prompts are just arrays of markdown files. Edit and compose instructions however you wish. It doesn't matter if you name your files `CLAUDE.md`, `AGENTS.md`, or `THE_FUTURE.md`.
- **Your Tools**: Aether agents get tools _exclusively_ via [MCP](https://modelcontextprotocol.io/) servers. Extend capabilities using _any_ language you like. Local and remote servers are supported (including OAuth). And Aether ships with 1st-party MCPs for file system operations, lsp integration, sub-agents, skills and more. Connect as many MCPs as you'd like without blowing out your context window as progressive discovery is built into the harness.
- **Your Model**: Anthropic, OpenAI, OpenRouter, DeepSeek, Gemini, Moonshot, ZAI, Llama.cpp, and Ollama are [supported out of the box](https://aether-agent.io/aether/configuration/llm-providers/). Bring your own provider via the `StreamingModelProvider` trait, or [alloy models together](https://aether-agent.io/aether/configuration/llm-providers/#alloying) to combine their strengths.
- **Your Interface**: 
  - **TUI**: Aether's TUI supports syntax highlighting, PR-style comments on git diffs to give feedback to your agent, custom themes and more.
  - **IDE/Editor**: Aether is an [ACP](https://agentclientprotocol.com/get-started/introduction) agent, so you can use it with any client that supports ACP (e.g. Zed).
  - **Headless**: Aether can be run as a headless agent that streams structured JSON logs.
  - **Rust library**: Aether can also be used as a Rust library to build agentic applications.
  
  
## Quick Start

### 1. **Install**

   **macOS** (Apple Silicon):

   ```bash
   brew install contextbridge/tap/aether
   ```

   **macOS / Linux** (x64, ARM64):

   ```bash
   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/contextbridge/aether/releases/latest/download/aether-agent-cli-installer.sh | sh
   ```

   **Any platform** (requires **Rust 1.85+**):

   ```bash
   cargo install aether-agent-cli
   ```

### 2. **Create your first agent**

   ```bash
   cd your-project
   aether agent new
   ```

   ```
   ✓ Created .aether/settings.json   — agent definitions (model, prompts, tools)
   ✓ Created .aether/mcp.json        — MCP server config
   ✓ Created .aether/SYSTEM.md       — base system prompt
   ✓ Created AGENTS.md               — project-level instructions
   ```
   
### 3. **Run it**

   - In a **TUI** — interactive terminal UI:

     ```bash
     aether
     ```
   
  - In an **editor/IDE** via [ACP](https://agentclientprotocol.com/get-started/introduction):

     ```bash
     aether acp
     ```

   - As a **headless** agent:

     ```bash
     aether headless "Refactor auth module"
     ```

## Using Aether as a library

Use `aether-agent-core` as a Rust library to build your own agent in ~25 lines. Bring your own model via the `StreamingModelProvider` trait, or alloy models together to round-robin across providers per turn.

1. **Add dependencies**

   ```bash
   cargo add aether-agent-core tokio
   ```

2. **Write your agent**

   ```rust
   use aether_core::{
       core::{Prompt, agent},
       events::{AgentMessage, UserMessage},
       mcp::{McpSpawnResult, mcp},
   };
   use llm::providers::anthropic::AnthropicProvider;
   use std::io::{self, Write};

   #[tokio::main]
   async fn main() -> Result<(), Box<dyn std::error::Error>> {
       // 1. Create a provider (reads ANTHROPIC_API_KEY from env)
       let llm = AnthropicProvider::new(None)?;

       // 2. Spawn MCP tool servers from one or more mcp.json files
       let McpSpawnResult { tool_definitions: tools, command_tx: mcp_tx, .. } =
           mcp().from_json_files(&["mcp.json"]).await?.spawn().await?;

       // 3. Build and spawn the agent
       let (tx, mut rx, _handle) = agent(llm)
           .system_prompt(Prompt::from_globs(vec!["AGENTS.md".into()], ".".into()))
           .tools(mcp_tx, tools)
           .spawn()
           .await?;

       // 4. Send a message and stream the response
       tx.send(UserMessage::text("Hello!")).await?;

       loop {
           match rx.recv().await {
               Some(AgentMessage::Text { chunk, is_complete, .. }) => {
                   if !is_complete { print!("{chunk}"); io::stdout().flush()?; }
               }
               Some(AgentMessage::Done) => break,
               Some(AgentMessage::Error { message }) => { eprintln!("Error: {message}"); break; }
               _ => {}
           }
       }
       Ok(())
   }
   ```

## Packages

| Package | Description |
|---------|-------------|
| [`aether-agent-core`](packages/aether-core) | Core agent library — LLM + prompt + tools in a loop ([docs](https://aether-agent.io/libraries/aether-core/agent-builder/)) |
| [`llm`](packages/llm) | Multi-provider LLM abstraction ([docs](https://aether-agent.io/libraries/llm/provider-interface/)) |
| [`wisp`](packages/wisp) | Terminal UI for AI agents, built on ACP ([docs](https://aether-agent.io/aether/terminal/overview/)) |
| [`aether-agent-cli`](packages/aether-cli) | Headless CLI and ACP server for editor integration ([docs](https://aether-agent.io/aether/running/headless/)) |
| [`mcp-servers`](packages/mcp-servers) | Pre-built MCP tool servers (coding, LSP, skills, tasks, sub-agents, survey) ([docs](https://aether-agent.io/aether/built-in-servers/coding/)) |
| [`crucible`](packages/crucible) | Automated testing (evals) for LLM agents ([docs](https://aether-agent.io/libraries/crucible/evals/)) |
| [`aether-lspd`](packages/aether-lspd) | LSP daemon — shares language servers across agents |
| [`aether-project`](packages/aether-project) | Project configuration and agent catalog from `.aether/settings.json` |

## License

MIT
