# aether-agent-cli

The Aether CLI ships as a single binary, **`aether`**, with subcommands for each mode of use:

- `aether` — interactive TUI (default when run with no args)
- `aether headless` — single-prompt headless run for scripting/CI
- `aether acp` — [Agent Client Protocol (ACP)](https://agentclientprotocol.com/overview/introduction) server for editor/IDE integration (e.g. Zed)
- `aether agent new|list|remove` — manage project agents
- `aether show-prompt` — print the fully-assembled system prompt (debugging)

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Install](#install)
- [Quick Start](#quick-start)
  - [Interactive TUI](#interactive-tui)
  - [Headless](#headless)
  - [ACP server](#acp-server)
- [Choosing a Model](#choosing-a-model)
- [Editor Integration (ACP)](#editor-integration-acp)
  - [Zed](#zed)
- [MCP Configuration](#mcp-configuration)
- [Slash Commands](#slash-commands)
- [Settings](#settings)
  - [Agents (Modes and Sub-agents)](#agents-modes-and-sub-agents)
- [Logs](#logs)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Install

Pick whichever fits your workflow:

```bash
# npm (cross-platform)
npm install -g @aether-agent/cli

# Homebrew (macOS / Linux)
brew install jcarver989/tap/aether

# Shell installer (macOS / Linux)
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/jcarver989/aether/releases/latest/download/aether-agent-cli-installer.sh | sh

# From source
cargo install aether-agent-cli
```

To build from a workspace checkout:

```bash
cargo build --release -p aether-agent-cli
# binary lands at target/release/aether
```

## Quick Start

### Interactive TUI

```bash
aether
```

If the project has no `.aether/settings.json`, the binary launches an onboarding wizard before starting the TUI.

### Headless

```bash
aether headless -m anthropic:claude-sonnet-4-5-20250929 "Refactor auth module"
```

Useful flags: `-a/--agent` (named agent from settings), `-C/--cwd`, `--system-prompt`, `--output text|json`, `--events tool_call,tool_result,...`, `--mcp-config <path>`, `--settings-file <path>` / `--settings-json <json>`. Pass the prompt as positional args, or pipe it on stdin.

### ACP server

```bash
aether acp --model anthropic:claude-sonnet-4-5-20250929 --mcp-config mcp.json
```

Flags: `--agent <name>` (mutually exclusive with `--model`), `--reasoning-effort low|medium|high|xhigh`, `--log-dir <path>`, plus the same `--settings-file` / `--settings-json` options.

## Choosing a Model

Aether supports multiple LLM providers using a `provider:model` string format:

| Provider | Example | Env var required |
|----------|---------|-----------------|
| Anthropic | `anthropic:claude-sonnet-4-5-20250929` | `ANTHROPIC_API_KEY` |
| OpenRouter | `openrouter:moonshotai/kimi-k2-thinking` | `OPENROUTER_API_KEY` |
| ZAI | `zai:GLM-4.6` | `ZAI_API_KEY` |
| Ollama | `ollama:llama3.2` | None (local) |
| Llama.cpp | `llamacpp` | None (local) |

## Editor Integration (ACP)

### Zed

Add to your Zed `settings.json` (Main Menu → "Open Settings File"):

```json
{
  "agent_servers": {
    "Aether Agent": {
      "command": "/path/to/aether",
      "args": [
        "acp",
        "--model",
        "zai:GLM-4.6",
        "--mcp-config",
        "/path/to/your/project/mcp.json"
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
- `command`: Full path to the `aether` binary (e.g. the result of `which aether` or `target/release/aether`)
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

- **coding** — Filesystem tools (read, write, bash, etc.) plus optional auto-read rules from configured `--rules-dir` paths
- **skills** — Slash commands and reusable prompts loaded from the configured `--dir` paths

## Slash Commands

Slash commands are markdown files served by the `skills` MCP server from any directory passed via `--dir`. Each entry is either a single `.md` file or a directory containing `SKILL.md` (plus optional supporting files). To make a prompt appear as a `/slash-command` in the TUI / ACP client, set `userInvocable: true` in its frontmatter.

**Example** `.aether/skills/plan.md`:

```markdown
---
description: Create a detailed implementation spec for a task
argumentHint: <task description>
userInvocable: true
---

You are an expert software architect. Create a comprehensive technical specification.

# Task
$ARGUMENTS
```

**Frontmatter fields:**
- `description` — shown in command pickers
- `argumentHint` — optional hint string for the argument
- `userInvocable` — exposes the prompt as a `/slash-command`
- `agentInvocable` — exposes the prompt as a skill that other agents can `get_skills` against
- `tags` — used by the `search_notes` / `list_skills` discovery surface

**Parameter syntax in the body:**
- `$ARGUMENTS` — full argument string (e.g. `/plan add user auth` → `add user auth`)
- `$1`, `$2`, `$3` — positional arguments

## Settings

Project-level agent configuration lives in `.aether/settings.json` at your project root. This file defines agents (modes and sub-agents), default prompts, and default MCP server configuration.

### Agents (Modes and Sub-agents)

Define agents with specific model, prompts, and tool configurations:

```json
{
  "agent": "planner",
  "prompts": [".aether/prompts/shared.md", "AGENTS.md"],
  "mcps": [".aether/mcp.json"],
  "agents": [
    {
      "name": "planner",
      "description": "Planner optimized for decomposition and sequencing",
      "model": "anthropic:claude-sonnet-4-5",
      "reasoningEffort": "high",
      "userInvocable": true,
      "agentInvocable": true
    },
    {
      "name": "researcher",
      "description": "Read-only research agent",
      "model": "anthropic:claude-sonnet-4-5",
      "userInvocable": false,
      "agentInvocable": true,
      "prompts": [".aether/prompts/researcher.md"],
      "mcps": [".aether/researcher-mcp.json"],
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
      "prompts": [".aether/prompts/coder.md"]
    }
  ]
}
```

- **`agent`** — Optional default user-invocable agent name.
- **Top-level `prompts`** — Ordered default prompt sources used by agents that do not define their own `prompts`. File paths can be written as strings; typed objects support `{ "type": "text", "text": "..." }`, `{ "type": "file", "path": "..." }`, and `{ "type": "glob", "pattern": "..." }`.
- **Top-level `mcps`** — Ordered default MCP config sources used by agents that do not define their own `mcps`. File paths can be written as strings; typed objects support `{ "type": "file", "path": "...", "proxy": false }` and inline `{ "type": "inline", "servers": { ... } }` entries.
- **Agent `prompts`** — Optional ordered prompt sources that override top-level `prompts` for that agent. Supports the same string shorthand and typed objects as top-level `prompts`.
- **Agent `mcps`** — Optional ordered MCP config sources that override top-level `mcps` for that agent. Supports the same string shorthand and typed objects as top-level `mcps`.
- **`userInvocable: true`** — Agent appears as a mode option in ACP clients (e.g., Wisp's Shift+Tab)
- **`agentInvocable: true`** — Agent can be spawned as a sub-agent
- **`tools`** — Filter which MCP tools the agent can use (optional). Supports `allow` (allowlist) and `deny` (blocklist) with trailing `*` wildcards. If both are set, `allow` is applied first, then `deny` removes from the result. Omit or leave empty to allow all tools.

You can scaffold settings interactively via `aether agent new`, list current agents with `aether agent list`, and remove one with `aether agent remove <name>`.

## Logs

ACP runs write logs to `--log-dir` (default: `/tmp/aether-acp-logs/`). Control verbosity with the `RUST_LOG` environment variable.
