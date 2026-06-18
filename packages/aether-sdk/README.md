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
  - [Writing evals with vitest](#writing-evals-with-vitest)
    - [Grading a run with an LLM judge](#grading-a-run-with-an-llm-judge)

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

The SDK depends on `@aether-agent/cli`, which bundles the `aether` binary for
your platform, so no separate install is required. Pass `binaryPath` to
`AetherSession.start()` if you want to point at a system or custom-built
`aether` instead (an absolute path or any name resolvable on `PATH`).

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

| Option               | Notes                                                                                                                         |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `agent`              | Mode name from `.aether/settings.json` (e.g. `planner`).                                                                      |
| `model`              | Direct model id (e.g. `anthropic:claude-sonnet-4-5`).                                                                         |
| `reasoningEffort`    | `"low"`, `"medium"`, `"high"`, `"xhigh"`.                                                                                     |
| `settings`           | Inline Aether settings object using the `.aether/settings.json` shape.                                                        |
| `settingsFile`       | Path to an alternate settings JSON file.                                                                                      |
| `cwd`                | Working directory for the spawned `aether acp` process.                                                                       |
| `binaryPath`         | Override the bundled `@aether-agent/cli` binary (absolute path or name on `PATH`).                                            |
| `tools`              | Closure-backed TypeScript tool groups keyed by Aether tool prefix.                                                            |
| `externalMcpServers` | External stdio/http/sse MCP servers keyed by Aether tool prefix.                                                              |
| `providers`          | Provider connection overrides, keyed by provider (for example `{ bedrock: { url: "http://127.0.0.1:8787", auth: "none" } }`). |
| `abortSignal`        | Cancel the active session and tear the subprocess down.                                                                       |

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

  for await (const _message of session.prompt(
    "Call custom__submit_answer with the final answer.",
  )) {
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
    return {
      outcome: { outcome: "selected", optionId: request.options[0].optionId },
    };
  },
});
```

`onElicitation` handles Aether's `_aether/elicitation` extension request.

## Writing evals with vitest

`@aether-agent/sdk/evals` runs a single Dockerized eval per test via the `aether`
CLI. `runEval` runs the agent and returns the outcome; you assert against it
directly with your test runner. `result.passed` reflects whether the agent ran to
completion — there are no built-in matchers, so check files, tool calls, and the
workspace yourself.

```ts
import { test, expect } from "vitest";
import { runEval } from "@aether-agent/sdk/evals";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

test("agent edits notes.txt", async () => {
  await using result = await runEval({
    docker: { image: "aether-sandbox:latest" },
    name: "edit-notes",
    task: {
      prompt: "Change the first line of notes.txt from alpha to beta",
      workspace: { files: { "notes.txt": "alpha\nalpha\n" } },
    },
  });

  expect(result.passed).toBe(true);
  // Assert against the retained workspace and the recorded tool calls.
  expect(await readFile(join(result.workspace.path, "notes.txt"), "utf8")).toBe(
    "beta\nalpha\n",
  );
  // Workspace is removed when `result` goes out of scope.
});
```

### Grading a run with an LLM judge

For judgments you can't express deterministically, ask a model to score a rubric
with `generate`, then let `judge` compute the final weighted score and
blocker status. Collect the transcript with `runEval`'s `onMessage` callback and
put evidence under `context` along with any diff or final file contents:

```ts
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test, expect } from "vitest";
import {
  generate,
  judge,
  runEval,
  type AgentMessage,
} from "@aether-agent/sdk/evals";

test("agent makes a maintainer-quality edit", async () => {
  const transcript: AgentMessage[] = [];
  await using result = await runEval(
    {
      docker: { image: "aether-sandbox:latest" },
      name: "edit-notes",
      task: { prompt: "Change the first line of notes.txt from alpha to beta", /* ... */ },
    },
    { onMessage: (message) => transcript.push(message) },
  );

  const grader = judge({
    instructions: "Grade strictly using only the provided transcript and files.",
    task: "Change the first line of notes.txt from alpha to beta",
    context: {
      transcript,
      files: {
        "notes.txt": await readFile(join(result.workspace.path, "notes.txt"), "utf8"),
      },
    },
    criteria: [
      {
        id: "correct-edit",
        description: "The final notes.txt content is exactly 'beta\\nalpha\\n'.",
        blocking: true,
        threshold: 1,
        weight: 3,
      },
      {
        id: "minimal-change",
        description: "No unrelated files or lines were changed.",
        blocking: true,
        threshold: 0.8,
        weight: 2,
      },
    ],
  });

  const response = await generate(grader.prompt, {
    model: "anthropic:claude-sonnet-4-5", // or set AETHER_LLM_MODEL
    schema: grader.schema,
  });
  const summary = grader.summarize(response);

  expect(summary.passed, summary.reason).toBe(true);
});
```

The model returns only normalized criterion scores and reasons. `judge`
checks that the response has exactly one result for each criterion, then computes
criterion pass/fail, weighted score, blocker failures, and the final summary.

The lower-level `generate` primitive also supports raw text responses:

```ts
import { generate } from "@aether-agent/sdk/evals";
import { z } from "zod";

const { text } = await generate("Summarize this diff in one sentence:\n" + diff, {
  model: "anthropic:claude-sonnet-4-5", // or set AETHER_LLM_MODEL
});

const verdict = await generate("Grade this run. Respond with passed and reason.", {
  model: "anthropic:claude-sonnet-4-5",
  schema: z.object({ passed: z.boolean(), reason: z.string() }),
});
```

Pass `{ keepWorkspace: true }` to retain the workspace on disk for debugging.

`runEval` also streams the agent's messages and the eval process's stderr while it runs. Pass
`onMessage` and `onStderr` callbacks to observe them:

```ts
await using result = await runEval(spec, {
  onMessage: (message) => {
    if (message.type === "text") process.stdout.write(message.chunk);
    if (message.type === "tool_call") console.error("tool:", message.request.name);
  },
  onStderr: (chunk) => process.stderr.write(chunk),
});
```

`message` is the generated `AgentMessage` union from `@aether-agent/sdk/evals`.
