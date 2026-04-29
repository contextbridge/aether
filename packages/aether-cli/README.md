# aether-agent-cli

Binary package containing Aether's two runnable entrypoints:

- **`aether-acp`** — [Agent Client Protocol (ACP)](https://agentclientprotocol.com/overview/introduction) server for editor/IDE integration (e.g. Zed)
- **`aether`** — Headless CLI for single-prompt usage

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quick Start](#quick-start)
  - [Build](#build)
  - [Run the CLI](#run-the-cli)
  - [Run the ACP server](#run-the-acp-server)
- [Choosing a Model](#choosing-a-model)
- [Editor Integration (ACP)](#editor-integration-acp)
  - [Zed](#zed)
- [MCP Configuration](#mcp-configuration)
- [Slash Commands](#slash-commands)
- [Settings](#settings)
  - [Agents (Modes and Sub-agents)](#agents-modes-and-sub-agents)
- [Logs](#logs)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Quick Start

### Build

From the workspace root:

```bash
cargo build --release -p aether-agent-cli
```

Binaries will be at `target/release/aether-acp` and `target/release/aether`.

### Run the CLI

```bash
cargo run -p aether-agent-cli --bin aether -- -m anthropic:claude-sonnet-4-5-20250929 "Refactor auth module"
```

### Run the ACP server

```bash
cargo run -p aether-agent-cli --bin aether-acp -- --model anthropic:claude-sonnet-4-5-20250929 --mcp-config mcp.json
```

## Choosing a Model

Aether supports multiple LLM providers using a `provider:model` string format:

| Provider | Example | Env var required |
|----------|---------|-----------------|
| Anthropic | `anthropic:claude-sonnet-4-5-20250929` | `ANTHROPIC_API_KEY` |
| `OpenRouter` | `openrouter:moonshotai/kimi-k2-thinking` | `OPENROUTER_API_KEY` |
| ZAI | `zai:GLM-4.6` | `ZAI_API_KEY` |
| Ollama | `ollama:llama3.2` | None (local) |
| Llama.cpp | `llamacpp` | None (local) |

## Editor Integration (ACP)

### Zed

Add to your Zed `settings.json` (Main Menu -> "Open Settings File"):

```json
{
  "agent_servers": {
    "Aether Agent": {
      "command": "/path/to/aether/target/release/aether-acp",
      "args": [
        "--model",
        "zai:GLM-4.6",
        "--mcp-config",
        "/path/to/aether/mcp.json"
      ],
      "env": {
        "RUST_LOG": "debug",
        "ZAI_API_KEY": "your-api-key-here"
      }
    }
  }
}
```

Then open the [Agent Panel](https://zed.dev/docs/ai/agent-panel) and select "New Aether Agent Thread".

**Important:** Update the paths and configuration:
- `command`: Full path to your built `aether-acp` binary
- `--mcp-config`: Path to your MCP configuration file
- Set the appropriate API key env var for your model provider

## MCP Configuration

The `mcp.json` file configures MCP tool servers:

```json
{
  "servers": {
    "coding": {
      "type": "in-memory",
      "args": ["--rules-dir", ".aether/skills", "--rules-dir", ".claude/rules"]
    },
    "skills": {
      "type": "in-memory",
      "args": [
        "--dir", ".aether/skills",
        "--dir", ".claude/skills",
        "--notes-dir", ".aether/notes"
      ]
    }
  }
}
```

- **coding** — Filesystem tools (read, write, bash, etc.) + optional automatic read rules from configured `--rules-dir` paths
- **skills** — Slash commands and reusable prompts from configured prompt directories

## Slash Commands

Create markdown files in `~/.aether/commands/` to define custom slash commands.

**Example** `~/.aether/commands/plan.md`:

```markdown
---
description: Create a detailed implementation spec for a task
---

You are an expert software architect. Create a comprehensive technical specification.

# Task
$ARGUMENTS
```

**Parameter syntax:**
- `$ARGUMENTS` — Full argument string (e.g., `/plan add user auth` -> "add user auth")
- `$1`, `$2`, `$3` — Positional arguments

## Settings

Project-level agent configuration is centralized in `.aether/settings.json` in your project root. This file defines agents (modes and sub-agents), prompts, and MCP server configuration.

### Agents (Modes and Sub-agents)

Define agents with specific model, prompts, and tool configurations:

```json
{
  "agent": "planner",
  "agents": [
    {
      "name": "planner",
      "description": "Planner optimized for decomposition and sequencing",
      "model": "anthropic:claude-sonnet-4-5",
      "reasoningEffort": "high",
      "userInvocable": true,
      "agentInvocable": true,
      "prompts": [
        { "type": "file", "path": ".aether/prompts/planner.md" },
        { "type": "file", "path": "AGENTS.md" }
      ],
      "mcp": [{ "type": "file", "path": ".aether/mcp/planner.json" }]
    },
    {
      "name": "researcher",
      "description": "Read-only research agent",
      "model": "anthropic:claude-sonnet-4-5",
      "userInvocable": false,
      "agentInvocable": true,
      "prompts": [{ "type": "file", "path": ".aether/prompts/researcher.md" }],
      "tools": {
        "allow": ["coding__grep", "coding__read_file", "coding__glob"],
        "deny": []
      }
    },
    {
      "name": "coder",
      "description": "Fast coding agent",
      "model": "deepseek:deepseek-chat",
      "userInvocable": true,
      "agentInvocable": false,
      "prompts": [{ "type": "file", "path": ".aether/prompts/coder.md" }]
    }
  ]
}
```

- **`agent`** — Optional default user-invocable agent name.
- **`userInvocable: true`** — Agent appears as a mode option in ACP clients (e.g., Wisp's Shift+Tab)
- **`agentInvocable: true`** — Agent can be spawned as a sub-agent
- **`prompts`** — Ordered prompt sources. Supports `{ "type": "text", "text": "..." }`, `{ "type": "file", "path": "..." }`, and `{ "type": "glob", "pattern": "..." }`.
- **`mcp`** — Ordered MCP config sources. Supports `{ "type": "file", "path": "...", "proxy": false }` and inline `{ "type": "inline", "servers": { ... } }` entries.
- **`tools`** — Filter which MCP tools the agent can use (optional). Supports `allow` (allowlist) and `deny` (blocklist) with trailing `*` wildcards. If both are set, `allow` is applied first, then `deny` removes from the result. Omit or leave empty to allow all tools.

## Logs

Logs are written to `--log-dir` (default: `/tmp/aether-acp-logs/`). Control verbosity with the `RUST_LOG` environment variable.
