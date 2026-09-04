<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [@aether-agent/sdk](#aether-agentsdk)
  - [Install](#install)
  - [Basic session](#basic-session)
  - [Multi-turn usage](#multi-turn-usage)
  - [SDK-hosted MCP tools with `mcp()`](#sdk-hosted-mcp-tools-with-mcp)
    - [Per-agent tools](#per-agent-tools)
    - [How `mcp()` is wired](#how-mcp-is-wired)
    - [Aether tool naming](#aether-tool-naming)
  - [Permission and elicitation hooks](#permission-and-elicitation-hooks)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

# @aether-agent/sdk

TypeScript SDK for the [Aether](https://aether-agent.io) agent. It spawns
`aether acp` under the hood and exposes one explicit stateful API:

- `AetherSession` — start an ACP session, send prompts, then close it
- `mcp()` — host closure-backed TypeScript tools as an MCP server and pass it to any agent via settings

## Install

```bash
pnpm add @aether-agent/sdk
# or: npm install @aether-agent/sdk
```

The SDK depends on `@aether-agent/cli`, which bundles the `aether` binary for
your platform, so no separate install is required. Pass `binaryPath` to
`AetherSession.start()` if you want to point at a system or custom-built
`aether` instead (an absolute path or any name resolvable on `PATH`).

## Basic session

`AetherSession` implements `Symbol.asyncDispose`, so the recommended pattern is
`await using` — the session closes and kills the subprocess automatically on
scope exit. SDK-hosted tool servers created with `mcp()` have their own
lifetime; see [`mcp()`](#sdk-hosted-mcp-tools-with-mcp).

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

| Option            | Notes                                                                                                                         |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `agent`           | Mode name from `.aether/settings.json` (e.g. `planner`).                                                                      |
| `model`           | Direct model id (e.g. `anthropic:claude-sonnet-4-5`).                                                                         |
| `reasoningEffort` | `"low"`, `"medium"`, `"high"`, `"xhigh"`.                                                                                     |
| `settings`        | Inline Aether settings object using the `.aether/settings.json` shape. SDK-hosted tools live here under `mcps`.               |
| `settingsFile`    | Path to an alternate settings JSON file.                                                                                      |
| `cwd`             | Working directory for the spawned `aether acp` process.                                                                       |
| `binaryPath`      | Override the bundled `@aether-agent/cli` binary (absolute path or name on `PATH`).                                            |
| `providers`       | Provider connection overrides, keyed by provider (for example `{ bedrock: { url: "http://127.0.0.1:8787", auth: "none" } }`). |
| `traceContext`    | A remote W3C `traceparent` with optional `tracestate`, or a standalone `traceId` for root spans without a parent.             |
| `abortSignal`     | Cancel the active session and tear the subprocess down.                                                                       |

`agent` and `model` are mutually exclusive. `settings` and `settingsFile` are
mutually exclusive. These are forwarded to the spawned `aether acp` process as
`--settings-json` and `--settings-file`, where the CLI resolves the initial
system prompt and tool filter before the session is constructed.

Provider connection overrides route a provider to a custom endpoint and can also
change auth behavior. Set `auth: "none"` only when a trusted proxy injects or
signs auth:

```ts
await AetherSession.start({
  model: "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0",
  providers: { bedrock: { url: "http://127.0.0.1:8787", auth: "none" } },
});
```

For Bedrock inference profiles, keep `model` as the Bedrock foundation model ID
and pass the profile ARN as the Bedrock provider request target:

```ts
await AetherSession.start({
  model: "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0",
  providers: {
    bedrock: {
      inferenceProfileArn:
        "arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000",
    },
  },
});
```

## Configuring telemetry

Telemetry content is opt-in per OpenTelemetry GenAI attribute:

```ts
await using session = await AetherSession.start({
  settings: {
    telemetry: {
      content: {
        systemInstructions: true,
        inputMessages: true,
        outputMessages: true,
        toolDefinitions: true,
        toolCalls: true,
      },
      otlp: { endpoint: "http://localhost:4318" },
    },
    agents: [],
  },
});
```

All content flags default to `false`. This replaces the former
`telemetry.captureContent` setting. See the [telemetry documentation](https://aether-agent.io/aether/settings/telemetry/)
for signal toggles, endpoint overrides, headers, validation, and sampling semantics.

## Correlating telemetry traces

```ts
await using continuedSession = await AetherSession.start({
  traceContext: {
    traceparent: "00-00112233445566778899aabbccddeeff-0123456789abcdef-01",
    tracestate: "vendor=value",
  },
});

await using rootSession = await AetherSession.start({
  traceContext: {
    traceId: "00112233445566778899aabbccddeeff",
  },
});
```

`traceContext` is also accepted by `runHeadless` and the lower-level ACP process options. A `traceparent` makes every turn a child of the propagated span. A standalone `traceId` instead creates root turn spans with that trace ID and no parent span. Telemetry must still be enabled in Aether settings.

## Tracking token usage and cost

Each provider call emits a typed `usage` message with per-call usage and the
cumulative session totals:

```ts
for await (const message of session.prompt("Implement the feature")) {
  if (message.type === "usage") {
    console.log(message.usage.tokens);
    console.log(message.usage.estimated_cost?.total_usd);
    console.log(message.usage.totals.estimated_usd);
  }
}
```

Costs are catalog-based USD estimates. `totals.estimated_usd` excludes calls
whose model pricing is unknown; `totals.unpriced_calls` reports how many were
excluded. Sub-agent usage includes agent and task identity in
`message.usage.source`.

## Multi-turn usage

```ts
await using session = await AetherSession.start({ cwd: process.cwd() });
for await (const m of session.prompt("First question")) console.log(m);
for await (const m of session.prompt("Follow-up")) console.log(m);
```

## SDK-hosted MCP tools with `mcp()`

`mcp()` creates a TypeScript MCP server. Tools run **in the calling Node process**, so closures, in-memory state, file handles, and database connections all work as you'd expect.

The returned handle implements `Symbol.asyncDispose`, so `await using` tears the server down on scope exit.

```ts
import { AetherSession, mcp, tool } from "@aether-agent/sdk";
import { z } from "zod";

function createSubmitTool() {
  let submitted: { answer: string } | null = null;

  return {
    tool: tool({
      name: "submit_answer",
      description: "Submit the final answer",
      inputSchema: { answer: z.string() },
      handler: async ({ answer }) => {
        submitted = { answer };
        return { content: [{ type: "text", text: "Submitted." }] };
      },
    }),
    getResult: () => submitted,
  };
}

const submit = createSubmitTool();
await using custom = await mcp({ name: "custom", tools: [submit.tool] });
{
  await using session = await AetherSession.start({
    cwd: process.cwd(),
    settings: {
      agents: [],
      mcps: [custom.spec],
    },
  });

  for await (const _message of session.prompt(
    "Call custom__submit_answer with the final answer.",
  )) {
    void _message;
  }
}

console.log(submit.getResult());
```

### Per-agent tools

A spec on the top-level `mcps` is available to every agent. Put it on a single
agent's `mcps` instead to scope those tools to that agent.

```ts
await using planner = await mcp({ name: "planner-tools", tools: [plan] });
await using reviewer = await mcp({ name: "reviewer-tools", tools: [review] });

await using session = await AetherSession.start({
  settings: {
    agents: [
      {
        name: "planner",
        description: "Planner",
        model: "anthropic:claude-sonnet-4-5",
        userInvocable: true,
        mcps: [planner.spec],
      },
      {
        name: "reviewer",
        description: "Reviewer",
        model: "anthropic:claude-sonnet-4-5",
        userInvocable: true,
        mcps: [reviewer.spec],
      },
    ],
  },
});
```

### How `mcp()` is wired

Each `mcp()` call starts a small **Streamable HTTP MCP server** on
`127.0.0.1:<random-port>` and returns its address as an inline `McpSourceSpec`.
Adding that spec to `settings.mcps` (or an agent's `mcps`) tells the spawned
`aether` process to connect to it. Each server is protected by:

- A random bearer token (`Authorization: Bearer …`) minted per `mcp()` call.
- DNS rebinding protection (host-header validation) provided by
  `createMcpExpressApp()`.

The server starts when you `await mcp(...)` and stops when the handle is
disposed — via `await using` scope exit or an explicit
`await handle[Symbol.asyncDispose]()`. Disposal is idempotent.

### Aether tool naming

Aether names MCP tools as `server__tool` internally. The `name` passed to `mcp()`
is the server prefix. If you register a tool named `submit_answer` under the
`custom` name, the agent sees it as `custom__submit_answer`. If your selected
agent has a restrictive tool allowlist in `.aether/settings.json`, include the
custom server pattern or leave the filter empty.

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
    return {
      outcome: { outcome: "selected", optionId: request.options[0].optionId },
    };
  },
});
```

`onElicitation` handles native ACP `elicitation/create` requests in form or URL
mode. It returns an ACP accept, decline, or cancel response. Without a handler,
the SDK does not advertise elicitation support. URL completion notifications
appear in the prompt stream as `elicitation_complete` messages.
