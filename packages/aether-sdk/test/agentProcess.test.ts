import { fileURLToPath } from "node:url";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { buildAetherAcpCommand } from "../src/agentProcess.js";
import { TRACE_CONTEXT, TRACE_ID_CONTEXT } from "./traceContext.js";

const FAKE_AETHER = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeAether.mjs",
);

describe("buildAetherAcpCommand()", () => {
  it("builds the aether acp command for an explicit binary", () => {
    expect(buildAetherAcpCommand({ binaryPath: FAKE_AETHER })).toEqual({
      command: FAKE_AETHER,
      args: ["acp"],
    });
  });

  it("adds ACP options JSON for settings, model, reasoning effort, providers, and log dir", () => {
    const command = buildAetherAcpCommand({
      binaryPath: FAKE_AETHER,
      settings: { credentialsStore: { type: "memory" }, agents: [] },
      model: "anthropic:claude-sonnet-4-5",
      reasoningEffort: "high",
      traceContext: TRACE_CONTEXT,
      providers: {
        zed: { auth: "none" },
        bedrock: {
          url: "http://localhost:8787",
          inferenceProfileArn: "arn:test",
        },
      },
      logDir: "/tmp/aether-logs",
    });

    expect(command.command).toBe(FAKE_AETHER);
    expect(command.args[0]).toBe("acp");
    expect(command.args[1]).toBe("--options-json");
    expect(JSON.parse(command.args[2]!)).toEqual({
      logDir: "/tmp/aether-logs",
      providers: {
        bedrock: {
          url: "http://localhost:8787",
          inferenceProfileArn: "arn:test",
        },
        zed: { auth: "none" },
      },
      settings: {
        credentialsStore: { type: "memory" },
        agents: [],
      },
      model: "anthropic:claude-sonnet-4-5",
      reasoningEffort: "high",
      traceContext: TRACE_CONTEXT,
    });
  });

  it("adds a trace-only context to ACP options JSON", () => {
    const command = buildAetherAcpCommand({
      binaryPath: FAKE_AETHER,
      traceContext: TRACE_ID_CONTEXT,
    });

    expect(JSON.parse(command.args[2]!)).toEqual({
      traceContext: TRACE_ID_CONTEXT,
    });
  });

  it("rejects mutually exclusive settings sources", () => {
    expect(() =>
      buildAetherAcpCommand({
        binaryPath: FAKE_AETHER,
        settings: { agents: [] },
        settingsFile: ".aether/settings.json",
      } as never),
    ).toThrow(/settings and settingsFile/);
  });

  it("rejects agent and model together", () => {
    expect(() =>
      buildAetherAcpCommand({
        binaryPath: FAKE_AETHER,
        agent: "planner",
        model: "anthropic:claude-sonnet-4-5",
      } as never),
    ).toThrow(/agent and model/);
  });

  it("rejects reasoning effort without a model", () => {
    expect(() =>
      buildAetherAcpCommand({
        binaryPath: FAKE_AETHER,
        reasoningEffort: "high",
      } as never),
    ).toThrow(/reasoningEffort requires model/);
  });
});
