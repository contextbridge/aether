#!/usr/bin/env node
// End-to-end probe: spawns the real `aether acp` binary, registers a
// closure-backed `weather__get_current` tool, then asks the agent to call it
// and report the result.
//
// Usage:
//   node scripts/e2e.mjs [--bin <path>] [--cwd <path>]
//                        [--model <id>] [--reasoning-effort <level>]
//                        [-- <prompt words...>]
//
// Defaults:
//   --bin   <repo-root>/target/debug/aether
//   --cwd   <repo-root>
//   --model anthropic:claude-sonnet-4-6
//   prompt  asks the agent to call weather__get_current for Tokyo
//
// Requires a real LLM API key (e.g. ANTHROPIC_API_KEY) in the environment;
// The probe uses an isolated inline agent with only the weather tool enabled.

import { parseArgs } from "node:util";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";
import { z } from "zod";
import { AetherSession, mcp, tool } from "../dist/index.js";

const out = (s) => process.stdout.write(s);
const err = (s) => process.stderr.write(s);

const SDK_DIR = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const REPO_ROOT = path.resolve(SDK_DIR, "..", "..");
const DEFAULT_BIN = path.join(REPO_ROOT, "target", "debug", "aether");
const DEFAULT_PROMPT =
  "Call the weather__get_current tool for Tokyo, then tell me the temperature " +
  "and conditions in one sentence.";

const FAKE_WEATHER = {
  Tokyo: { tempC: 18, conditions: "light rain" },
  "San Francisco": { tempC: 14, conditions: "foggy" },
  London: { tempC: 9, conditions: "overcast" },
};

let weatherCalls = 0;
const getWeather = tool({
  name: "get_current",
  description:
    "Get the current weather (temperature in Celsius and conditions) for a city.",
  inputSchema: { city: z.string().describe("City name, e.g. 'Tokyo'") },
  handler: async ({ city }) => {
    weatherCalls += 1;
    const data = FAKE_WEATHER[city] ?? { tempC: 21, conditions: "clear" };
    const text = `Weather in ${city}: ${data.tempC}°C, ${data.conditions}.`;
    out(`[weather__get_current] city=${city} -> ${text}\n`);
    return { content: [{ type: "text", text }] };
  },
});

const { values, positionals } = parseArgs({
  options: {
    bin: { type: "string" },
    cwd: { type: "string" },
    model: { type: "string" },
    "reasoning-effort": { type: "string" },
  },
  allowPositionals: true,
});

const binaryPath = values.bin ?? process.env.AETHER_BIN ?? DEFAULT_BIN;
const cwd = values.cwd ?? REPO_ROOT;
const prompt = positionals.length > 0 ? positionals.join(" ") : DEFAULT_PROMPT;

if (!existsSync(binaryPath)) {
  err(
    `aether binary not found at ${binaryPath}\n` +
      `Build it first:  cargo build -p aether-agent-cli\n` +
      `Or pass --bin <path> / set AETHER_BIN=<path>.\n`,
  );
  process.exit(2);
}

const agentSelection = {
  model: values.model ?? "anthropic:claude-sonnet-4-6",
  ...(values["reasoning-effort"]
    ? { reasoningEffort: values["reasoning-effort"] }
    : {}),
};

out(`> aether: ${binaryPath}\n`);
out(`> cwd:    ${cwd}\n`);
if (agentSelection) out(`> select: ${JSON.stringify(agentSelection)}\n`);
out(`> prompt: ${prompt}\n\n`);

await using weather = await mcp({ name: "weather", tools: [getWeather] });
await using session = await AetherSession.start({
  binaryPath,
  cwd,
  agent: "sdk-e2e",
  settings: {
    agents: [
      {
        name: "sdk-e2e",
        description: "SDK end-to-end tool probe",
        ...agentSelection,
        userInvocable: true,
        prompts: [
          {
            type: "text",
            text: "Use the provided weather tool to answer the user.",
          },
        ],
        tools: { allow: ["weather__*"] },
        mcps: [weather.spec],
      },
    ],
  },
});

out(`> session: ${session.sessionId}\n\n`);

let stopReason = null;
let sawIdle = false;
for await (const message of session.prompt(prompt)) {
  if (message.type === "session_update") {
    const update = message.update;
    if (update.sessionUpdate === "agent_message_chunk") {
      const c = update.content;
      if (c.type === "text") out(c.text);
    } else if (update.sessionUpdate === "agent_thought_chunk") {
      const c = update.content;
      if (c.type === "text") out(c.text);
    } else if (update.sessionUpdate === "tool_call_update") {
      if (update.title) out(`\n[tool_call] ${update.title}\n`);
      if (update.status) out(`[tool_call] ${update.status}\n`);
    } else if (
      update.sessionUpdate === "state_update" &&
      update.state === "idle"
    ) {
      sawIdle = true;
    }
    if (
      (update.sessionUpdate === "agent_message_chunk" ||
        update.sessionUpdate === "agent_thought_chunk") &&
      !update.messageId
    ) {
      throw new Error("ACP v2 chunk missing messageId");
    }
  } else if (message.type === "result") {
    if (!sawIdle) throw new Error("Result arrived before idle state_update");
    stopReason = message.stopReason;
  } else if (message.type === "error") {
    err(`\n[error] ${String(message.error)}\n`);
    process.exitCode = 1;
  }
}

out(`\n\n> stop_reason:   ${stopReason ?? "<none>"}\n`);
out(`> weather_calls: ${weatherCalls}\n`);
if (
  !stopReason ||
  (stopReason !== "end_turn" && stopReason !== "max_turn_requests")
) {
  process.exitCode = 1;
}
if (weatherCalls === 0) {
  err("agent did not call weather__get_current\n");
  process.exitCode = process.exitCode ?? 1;
}
