import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import type { AgentMessage } from "../../src/generated/eval-types.js";
import type {
  Agent,
  AgentConfig,
  AgentRunResult,
} from "../../src/evals/index.js";
import { FakeAgent, Task, Workspace } from "../../src/evals/index.js";

describe("Task.run", () => {
  it("creates the workspace, runs the agent, and returns a TS-native result", async () => {
    const result = await (
      await task()
    ).run(new FakeAgent([TOOL_RESULT, { type: "done" }]));

    expect(result.passed).toBe(true);
    expect(result.toolCalls).toEqual([
      {
        name: "weather__get_current",
        arguments: { city: "Tokyo" },
        rawArguments: '{"city":"Tokyo"}',
      },
    ]);
    expect(result.transcript).toHaveLength(2);
    expect(existsSync(result.workspace.path)).toBe(true);
    expect(result.workspace.rootPath).toBe(result.workspace.path);
    expect(
      await readFile(join(result.workspace.path, "notes.txt"), "utf8"),
    ).toBe("alpha\n");

    await result.workspace.cleanup();
    expect(existsSync(result.workspace.path)).toBe(false);
  });

  it("forwards the workspace and task prompt to the agent", async () => {
    const agent = new CapturingAgent();

    await using result = await (await task()).run(agent);

    expect(agent.config?.taskPrompt).toBe("do the thing");
    expect(agent.config?.workspace.rootPath).toBe(result.workspace.rootPath);
  });

  it("reports passed=false when the terminal message is not `done`", async () => {
    await using result = await (
      await task()
    ).run(new FakeAgent([{ type: "error", message: "boom" }]));

    expect(result.passed).toBe(false);
  });

  it("invokes onMessage for each agent message as it streams", async () => {
    const seen: AgentMessage[] = [];

    await using result = await (
      await task()
    ).run(new FakeAgent([TOOL_RESULT, { type: "done" }]), {
      onMessage: (message) => seen.push(message),
    });

    expect(result.passed).toBe(true);
    expect(seen).toHaveLength(2);
    expect(seen[0]).toMatchObject({ type: "tool_result" });
  });

  it("removes the workspace at the end of an `await using` scope", async () => {
    let workspacePath: string;
    {
      await using result = await (await task()).run(FakeAgent.success());
      workspacePath = result.workspace.path;
      expect(existsSync(workspacePath)).toBe(true);
    }
    expect(existsSync(workspacePath)).toBe(false);
  });

  it("cleans up the workspace when the agent throws", async () => {
    const created = await task();
    const workspacePath = created.workspace.path;

    await expect(
      created.run(new ThrowingAgent("agent crashed")),
    ).rejects.toThrow("agent crashed");
    expect(existsSync(workspacePath)).toBe(false);
  });
});

const TOOL_RESULT: AgentMessage = {
  type: "tool_result",
  model_name: "fake",
  result: {
    id: "call_1",
    name: "weather__get_current",
    arguments: '{"city":"Tokyo"}',
    result: "sunny",
  },
};

async function task(): Promise<Task> {
  return new Task(
    "do the thing",
    await Workspace.fromFiles({ "notes.txt": "alpha\n" }),
  );
}

class CapturingAgent implements Agent {
  config?: AgentConfig;

  async run(config: AgentConfig): Promise<AgentRunResult> {
    this.config = config;
    return { transcript: [{ type: "done" }], stderr: "" };
  }
}

class ThrowingAgent implements Agent {
  constructor(private readonly message: string) {}

  run(): Promise<AgentRunResult> {
    return Promise.reject(new Error(this.message));
  }
}
