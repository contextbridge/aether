<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [@aether-agent/sdk](#aether-agentsdk)
  - [Install](#install)
  - [Basic session](#basic-session)
  - [Multi-turn usage](#multi-turn-usage)
  - [Closure-backed custom tool](#closure-backed-custom-tool)
    - [How closure-backed tools are wired](#how-closure-backed-tools-are-wired)
    - [Aether tool naming](#aether-tool-naming)
  - [External MCP servers](#external-mcp-servers)
  - [Permission and elicitation hooks](#permission-and-elicitation-hooks)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

# @aether-agent/sdk

TypeScript SDK for the [Aether](https://aether-agent.io) agent. It spawns
`aether acp` under the hood and exposes one explicit stateful API:

- `AetherSession` — start an ACP session, send prompts, then close it
- `tool()` plus `tools: { prefix: [...] }` — register **closure-backed TypeScript
  tools** that the agent can call as MCP tools

## Install

```bash
pnpm add @aether-agent/sdk
# or: npm install @aether-agent/sdk
```

You also need the `aether` CLI on your `PATH` or an explicit `aetherPath`.
See the [Aether install docs](https://aether-agent.io/getting-started/overview/).

## Basic session

`AetherSession` implements `Symbol.asyncDispose`, so the recommended pattern is
`await using` — the session closes (kills the subprocess, tears down MCP
servers) automatically on scope exit:

```ts
import { AetherSession } from "@aether-agent/sdk";

await using session = await AetherSession.start({
  cwd: "/path/to/repo",
  agent: "planner",
});

for await (const message of session.prompt("Find TODOs in this repo")) {
  if (message.type === "session_update") {
    console.log(message.update);
  }
}
```

If your runtime predates explicit resource management, call `session.close()`
yourself in a `finally` block.

`AetherSessionOptions` lets you pick the initial agent or model:

| Option            | Notes                                                                |
| ----------------- | -------------------------------------------------------------------- |
| `agent`           | Mode name from `.aether/settings.json` (e.g. `planner`).             |
| `model`           | Direct model id (e.g. `anthropic:claude-sonnet-4-5`).                |
| `reasoningEffort` | `"low"`, `"medium"`, `"high"`, `"xhigh"`.                            |
| `settings`        | Inline Aether settings object using the `.aether/settings.json` shape. |
| `settingsFile`    | Path to an alternate settings JSON file.                              |
| `cwd`             | Working directory for the spawned `aether acp` process.              |
| `tools`           | Closure-backed TypeScript tool groups keyed by Aether tool prefix.   |
| `externalMcpServers` | External stdio/http/sse MCP servers keyed by Aether tool prefix.   |
| `abortSignal`     | Cancel the active session and tear the subprocess down.              |

`agent` and `model` are mutually exclusive. `settings` and `settingsFile` are
mutually exclusive. These are forwarded to the spawned `aether acp` process as
`--settings-json` and `--settings-file`, where the CLI resolves the initial
system prompt and tool filter before the session is constructed.

## Multi-turn usage

```ts
await using session = await AetherSession.start({ cwd: process.cwd() });
for await (const m of session.prompt("First question")) console.log(m);
for await (const m of session.prompt("Follow-up")) console.log(m);
```

## Closure-backed custom tool

```ts
import { AetherSession, tool } from "@aether-agent/sdk";
import { z } from "zod";

function createSubmitTool() {
  let submitted: { answer: string } | null = null;

  return {
    tool: tool({
      name: "submit_answer",
      description: "Submit the final answer",
      input: { answer: z.string() },
      handler: async ({ answer }) => {
        submitted = { answer };
        return { content: [{ type: "text", text: "Submitted." }] };
      },
    }),
    getResult: () => submitted,
  };
}

const submit = createSubmitTool();
{
  await using session = await AetherSession.start({
    cwd: process.cwd(),
    tools: {
      custom: [submit.tool],
    },
  });

  for await (const _message of session.prompt("Call custom__submit_answer with the final answer.")) {
    void _message;
  }
}

console.log(submit.getResult());
```

The handler runs **in the calling Node process**, so closures, in-memory state,
file handles, and database connections all work as you'd expect. Add more
prefixes when you want multiple Aether tool namespaces:

```ts
tools: {
  recommendations: [submitRecommendations.tool],
  review: [approve.tool, reject.tool],
}
```

### How closure-backed tools are wired

To preserve TypeScript closures, each entry in `tools` starts a small
**Streamable HTTP MCP server** on `127.0.0.1:<random-port>` and tells
`aether acp` to connect there via ACP's `mcpServers` field. Each server is
protected by:

- A per-session random bearer token (`Authorization: Bearer …`).
- DNS rebinding protection (host-header validation) provided by
  `createMcpExpressApp()`.

The token is generated fresh per tool group on each `AetherSession.start()` call
and torn down when `session.close()` runs.

### Aether tool naming

Aether names MCP tools as `server__tool` internally. The `tools` object key is
the server prefix. If you register a tool named `submit_answer` under the
`custom` key, the agent will see it as `custom__submit_answer`. If your selected
agent has a restrictive tool allowlist in `.aether/settings.json`, include the
custom server pattern or leave the filter empty.

## External MCP servers

`externalMcpServers` accepts standard external server shapes, which are
forwarded to Aether unchanged. Object keys become Aether MCP server prefixes:

```ts
externalMcpServers: {
  filesystem: { type: "stdio", command: "uvx", args: ["mcp-server-filesystem", "/path"] },
  remote: {
    type: "http",
    url: "https://mcp.example.com/mcp",
    headers: { Authorization: "Bearer …" },
  },
  legacy: { type: "sse", url: "https://mcp.example.com/sse" },
}
```

## Permission and elicitation hooks

By default the SDK auto-accepts the first `allow_*` permission option — this is
the exported `autoApprovePermissions` handler, suitable for trusted/dev
contexts. For untrusted agents or production hosts, supply your own handler:

```ts
import { AetherSession, autoApprovePermissions } from "@aether-agent/sdk";

// Explicit auto-approve (same as the default).
await AetherSession.start({ onPermissionRequest: autoApprovePermissions });

// Custom policy.
await AetherSession.start({
  onPermissionRequest: async (request) => {
    return { outcome: { outcome: "selected", optionId: request.options[0].optionId } };
  },
});
```

`onElicitation` handles Aether's `_aether/elicitation` extension request.
