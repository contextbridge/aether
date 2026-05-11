# mcp-servers

Pre-built [MCP](https://modelcontextprotocol.io/) tool servers for Aether agents. Each server runs in-process and is gated behind a feature flag so you only compile what you need.

| Feature | Server | What it provides |
|---------|--------|-----------------|
| `coding` | [`CodingMcp`](src/coding/README.md) | File read/write/edit, bash, grep, find, LSP integration, web fetch/search |
| `skills` | [`SkillsMcp`](src/skills/README.md) | Slash commands and reusable skill prompts |
| `tasks` | [`TasksMcp`](src/tasks/README.md) | Hierarchical task management with dependencies |
| `subagents` | [`SubAgentsMcp`](src/subagents/README.md) | Spawn and orchestrate sub-agents |
| `survey` | [`SurveyMcp`](src/survey/README.md) | Human-in-the-loop elicitation (ask the user questions) |
| `plan` | [`PlanMcp`](src/plan/README.md) | Submit markdown plans for approval via native elicitation |

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Documentation](#documentation)
- [Using with Aether (mcp.json)](#using-with-aether-mcpjson)
- [Programmatic Usage](#programmatic-usage)
- [Server Documentation](#server-documentation)
- [Feature Flags](#feature-flags)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Documentation

Full API documentation is available on [docs.rs](https://docs.rs/aether-mcp-servers).

Key entry points:
- [`CodingMcp`] -- file I/O, shell, search, and LSP tools
- [`CodingTools`](coding::CodingTools) -- trait for custom tool backends
- [`LspRegistry`](lsp::LspRegistry) -- manages LSP daemon connections
- [`TasksMcp`](tasks::TasksMcp) -- hierarchical task management
- [`SkillsMcp`](skills::SkillsMcp) -- skill prompts and slash commands
- [`SubAgentsMcp`](subagents::SubAgentsMcp) -- sub-agent orchestration
- [`SurveyMcp`](survey::SurveyMcp) -- structured user input collection
- [`PlanMcp`](plan::PlanMcp) -- plan review and approval workflow
- [`McpBuilderExt`] -- register all servers in one call

## Using with Aether (mcp.json)

These servers use Aether's `in-memory` transport type -- they run inside your agent process, not as separate subprocesses. Wire them up in `mcp.json`:

```json
{
  "servers": {
    "coding": {
      "type": "in-memory",
      "args": ["--rules-dir", ".aether/skills"]
    },
    "skills": {
      "type": "in-memory",
      "args": ["--dir", ".aether/skills", "--notes-dir", ".aether/notes"]
    },
    "tasks": {
      "type": "in-memory"
    },
    "subagents": {
      "type": "in-memory",
      "args": ["--project-root", "."]
    },
    "plan": {
      "type": "in-memory"
    }
  }
}
```

Each server key must match a factory registered with `McpBuilder::register_in_memory_server()`. The `args` array is parsed as CLI flags by each server:

| Server | Flag | Default | Description |
|--------|------|---------|-------------|
| `coding` | `--root-dir <path>` | cwd | Workspace root for LSP and file operations |
| `coding` | `--rules-dir <path>` (repeatable) | none | Explicit prompt directories for automatic read-triggered rules |
| `coding` | `--disable-lsp` | enabled | Disable LSP-backed tools and `aether-lspd` daemon connections |
| `skills` | `--dir <path>` (repeatable) + `--notes-dir <path>` | required | Prompt directories to scan and directory for persisted notes |
| `tasks` | `--dir <path>` | `.` | Base directory for task storage (creates `.aether-tasks/` inside) |
| `subagents` | `--project-root <path>` (alias: `--dir`) | `.` | Project root containing optional `.aether/settings.json` authored agents |

To register factories and load the config:

```rust,ignore
use aether_core::mcp::mcp;
use futures::FutureExt;
use mcp_servers::{CodingMcp, PlanMcp, SkillsMcp, SubAgentsMcp, TasksMcp};

let builder = mcp()
    .register_in_memory_server("coding", Box::new(|_args| {
        async move { CodingMcp::new().into_dyn() }.boxed()
    }))
    .register_in_memory_server("skills", Box::new(|args| {
        async move { SkillsMcp::from_args(args).unwrap().into_dyn() }.boxed()
    }))
    .register_in_memory_server("tasks", Box::new(|args| {
        async move { TasksMcp::from_args(args).unwrap().into_dyn() }.boxed()
    }))
    .register_in_memory_server("subagents", Box::new(|args| {
        async move { SubAgentsMcp::from_args(args).unwrap().into_dyn() }.boxed()
    }))
    .register_in_memory_server("plan", Box::new(|_args| {
        async move { PlanMcp::new().into_dyn() }.boxed()
    }));

// Load mcp.json -- matches server keys to registered factories
let builder = builder.from_json_files(&["mcp.json"]).await?;
```

## Programmatic Usage

Add to your `Cargo.toml`:

```toml
# Everything
mcp-servers = { path = "../mcp-servers" }

# Just coding tools
mcp-servers = { path = "../mcp-servers", default-features = false, features = ["coding"] }
```

Create and start servers directly:

```rust,ignore
use mcp_servers::{CodingMcp, SkillsMcp, TasksMcp};
use rmcp::ServiceExt;

// Create a coding server
let server = CodingMcp::new()
    .with_root_dir("/my/project".into())
    .into_dyn();

// Or with LSP support
let server = CodingMcp::new()
    .with_root_dir("/my/project".into())
    .with_lsp("/my/project".into())
    .into_dyn();
```

## Server Documentation

- [`CodingMcp`](src/coding/README.md)
- [`SkillsMcp`](src/skills/README.md)
- [`TasksMcp`](src/tasks/README.md)
- [`SubAgentsMcp`](src/subagents/README.md)
- [`SurveyMcp`](src/survey/README.md)
- [`PlanMcp`](src/plan/README.md)

---

## Feature Flags

- **`default`** -- coding, skills, tasks, subagents, and plan servers
- **`coding`** -- file ops, bash, LSP, web tools
- **`skills`** -- slash commands and prompts
- **`tasks`** -- task tracking (no dependency on `coding`)
- **`subagents`** -- sub-agent spawning (implies `coding`, `skills`, `tasks`, `survey`, `plan`)
- **`survey`** -- structured human elicitation tooling
- **`plan`** -- markdown plan submission and approval workflow
- **`all`** -- explicit alias enabling all built-in servers

