import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import type { AgentEvent } from "@aether-agent/sdk";
import type { Agent } from "../src/index.js";
import {
  FakeAgent,
  Task,
  ToolCall,
  Transcript,
  TranscriptError,
  turnEnded,
  Workspace,
} from "../src/index.js";

describe("Transcript", () => {
  it("collects the agent stream and exposes transcript summaries", async () => {
    await using workspace = await Workspace.fromFiles({
      "notes.txt": "alpha\n",
    });
    const trace = await Transcript.fromStream(
      new FakeAgent([
        TOOL_RESULT,
        turnEnded(),
      ]).run(new Task("do the thing")),
    );

    expect(trace.events.at(-1)?.category).toBe("turn");
    expect(trace.allToolCalls()).toEqual([
      new ToolCall("weather__get_current", '{"city":"Tokyo"}'),
    ]);
    expect(trace.toolCalled("weather__get_current")).toBe(true);
    expect(trace.toolCallCount("weather__get_current")).toBe(1);
    expect(trace.events).toHaveLength(2);
    expect(existsSync(workspace.path)).toBe(true);
    expect(workspace.rootPath).toBe(workspace.path);
    expect(await readFile(join(workspace.path, "notes.txt"), "utf8")).toBe(
      "alpha\n",
    );
  });

  it("forwards the task prompt to the agent", async () => {
    const agent = new CapturingAgent();

    await Transcript.fromStream(agent.run(new Task("do the thing")));

    expect(agent.taskPrompt).toBe("do the thing");
  });

  it("records failed terminal turns", async () => {
    const trace = await Transcript.fromStream(
      new FakeAgent([
        turnEnded({ status: "failed", error: "boom" }),
      ]).run(new Task("do the thing")),
    );

    expect(trace.events.at(-1)).toMatchObject({
      category: "turn",
      event: { type: "ended", outcome: { status: "failed" } },
    });
  });

  it("add supports custom observation while walking the stream", async () => {
    const seen: AgentEvent[] = [];
    const trace = new Transcript();

    for await (const message of new FakeAgent([
      TOOL_RESULT,
      turnEnded(),
    ]).run(new Task("do the thing"))) {
      seen.push(message);
      trace.add(message);
    }

    expect(trace.events.at(-1)?.category).toBe("turn");
    expect(seen).toHaveLength(2);
    expect(seen[0]).toMatchObject({
      category: "tool",
      event: { type: "result" },
    });
  });

  it("leaves workspace cleanup with the caller", async () => {
    let workspacePath: string;
    {
      await using workspace = await Workspace.empty();
      workspacePath = workspace.path;
      await Transcript.fromStream(
        FakeAgent.success().run(new Task("do the thing")),
      );
      expect(existsSync(workspacePath)).toBe(true);
    }
    expect(existsSync(workspacePath!)).toBe(false);
  });

  it("throws TranscriptError carrying the partial transcript when the agent fails", async () => {
    const workspace = await Workspace.empty();
    const workspacePath = workspace.path;

    await expect(
      Transcript.fromStream(
        new ThrowingAgent("agent crashed").run(new Task("do the thing")),
      ),
    ).rejects.toBeInstanceOf(TranscriptError);
    expect(existsSync(workspacePath)).toBe(true);
    await workspace.cleanup();
    expect(existsSync(workspacePath)).toBe(false);
  });

  it("TranscriptError exposes the partial transcript and cause", async () => {
    let captured: TranscriptError | undefined;
    await Transcript.fromStream(
      new ThrowingAgent("agent crashed").run(new Task("do the thing")),
    ).catch((err: unknown) => {
      if (err instanceof TranscriptError) captured = err;
    });

    expect(captured).toBeInstanceOf(TranscriptError);
    expect(captured!.transcript).toBeInstanceOf(Transcript);
    expect(String(captured!.cause)).toContain("agent crashed");
  });
});

const TOOL_RESULT: AgentEvent = {
  category: "tool",
  event: {
    type: "result",
    result: {
      id: "call_1",
      name: "weather__get_current",
      arguments: '{"city":"Tokyo"}',
      result: "sunny",
    },
  },
};

class CapturingAgent implements Agent {
  taskPrompt?: string;

  async *run(task: Task): AsyncIterable<AgentEvent> {
    this.taskPrompt = task.prompt;
    yield* FakeAgent.success().run(task);
  }
}

class ThrowingAgent implements Agent {
  constructor(private readonly message: string) {}

  async *run(_task: Task): AsyncIterable<AgentEvent> {
    throw new Error(this.message);
  }
}
