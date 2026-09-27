import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import type { AgentEvent } from "@aether-agent/sdk";
import {
  FakeAgent,
  Task,
  Transcript,
  turnEnded,
  Workspace,
} from "../src/index.js";
import { eventName } from "./logMessage.js";

describe("FakeAgent", () => {
  it("success() ends with a done message", async () => {
    const result = await Transcript.fromStream(
      FakeAgent.success().run(new Task("t")),
    );

    expect(result.events.map(eventName)).toEqual([
      "message:text",
      "turn:ended",
    ]);
  });

  it("withToolCall() streams tool_result, text, then done", async () => {
    const result = await Transcript.fromStream(
      FakeAgent.withToolCall("bash", "ok").run(new Task("t")),
    );

    expect(result.events.map(eventName)).toEqual([
      "tool:result",
      "message:text",
      "turn:ended",
    ]);
  });

  it("writesFile() writes into the workspace, including nested paths", async () => {
    const ws = await workspace();

    await Transcript.fromStream(
      FakeAgent.writesFile("nested/hello.txt", "hello")
        .withWorkspace(ws)
        .run(new Task("t")),
    );

    expect(await readFile(join(ws.path, "nested/hello.txt"), "utf8")).toBe(
      "hello",
    );
  });

  it("add supports observing each streamed message", async () => {
    const seen: string[] = [];
    const trace = new Transcript();

    for await (const message of FakeAgent.success().run(new Task("t"))) {
      seen.push(eventName(message));
      trace.add(message);
    }

    expect(seen).toEqual(["message:text", "turn:ended"]);
    expect(trace.events.map(eventName)).toEqual(seen);
  });

  it("context_usage messages in the transcript flow into usage", async () => {
    const usageMessage: AgentEvent = {
      category: "context",
      event: {
        type: "usage_updated",
        usage: {
          usage_ratio: 0.5,
          context_limit: 200_000,
          input_tokens: 200,
        },
      },
    };
    const agent = new FakeAgent([
      {
        category: "message",
        event: {
          type: "text",
          message_id: "fake_1",
          chunk: "ok",
          is_complete: true,
        },
      },
      usageMessage,
      turnEnded(),
    ]);

    const result = await Transcript.fromStream(agent.run(new Task("t")));

    expect(result.events.map((event) => event.category)).toContain("context");
    expect(result.usage()).toEqual({
      usage_ratio: 0.5,
      context_limit: 200_000,
      input_tokens: 200,
    });
  });
});

const created: Workspace[] = [];

afterEach(async () => {
  for (const ws of created.splice(0)) await ws.cleanup();
});

async function workspace(): Promise<Workspace> {
  const ws = await Workspace.empty();
  created.push(ws);
  return ws;
}
