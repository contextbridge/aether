import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { z } from "zod";

import { runHeadless } from "../src/headless.js";
import { mcp } from "../src/mcp/index.js";
import { tool } from "../src/tool.js";
import { TRACE_ID_CONTEXT } from "./traceContext.js";

const FAKE_AETHER = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeAether.mjs",
);

let tempDirs: string[] = [];

afterEach(async () => {
  await Promise.all(
    tempDirs.map((dir) => rm(dir, { recursive: true, force: true })),
  );
  tempDirs = [];
});

describe("runAetherHeadless()", () => {
  it("passes SDK-hosted mcp() sources through to headless via settings.mcps", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "aether-sdk-headless-"));
    tempDirs.push(dir);
    const logFile = path.join(dir, "fake-aether.jsonl");
    const submit = tool({
      name: "submit",
      description: "submit",
      inputSchema: { value: z.string() },
      handler: async () => ({ content: [{ type: "text", text: "ok" }] }),
    });

    await using weather = await mcp({ name: "weather", tools: [submit] });

    const result = await runHeadless({
      binaryPath: FAKE_AETHER,
      prompt: "call the tool",
      model: "anthropic:claude-sonnet-4-5",
      settings: { agents: [], mcps: [weather.spec] },
      providers: {
        bedrock: {
          url: "http://127.0.0.1:8787",
          auth: "none",
          inferenceProfileArn: "arn:test",
        },
      },
      output: "json",
      events: ["tool_call", "turn_ended"],
      traceContext: TRACE_ID_CONTEXT,
      env: { ...process.env, FAKE_AETHER_LOG_FILE: logFile },
    });

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("call the tool");

    const log = JSON.parse((await readFile(logFile, "utf8")).trim());
    expect(log.event).toBe("headless");
    const optionsIndex = log.args.indexOf("--options-json");
    expect(optionsIndex).toBeGreaterThan(0);
    const options = JSON.parse(log.args[optionsIndex + 1]);
    expect(options).toMatchObject({
      prompt: "call the tool",
      settings: { agents: [] },
      model: "anthropic:claude-sonnet-4-5",
      output: "json",
      events: ["tool_call", "turn_ended"],
      traceContext: TRACE_ID_CONTEXT,
      providers: {
        bedrock: {
          url: "http://127.0.0.1:8787",
          auth: "none",
          inferenceProfileArn: "arn:test",
        },
      },
    });
    const inlineSource = options.settings.mcps[0];
    expect(inlineSource.type).toBe("inline");
    const weatherServer = inlineSource.servers.weather;
    expect(weatherServer).toMatchObject({ type: "http" });
    expect(weatherServer.headers.Authorization).toMatch(/^Bearer .+$/);
    expect(options).not.toHaveProperty("mcpConfig");
  });

  it("rejects conflicting options", async () => {
    await expect(
      runHeadless({
        binaryPath: FAKE_AETHER,
        prompt: "hello",
        agent: "planner",
        model: "anthropic:claude-sonnet-4-5",
      } as never),
    ).rejects.toThrow(/agent and model/);
  });
});
