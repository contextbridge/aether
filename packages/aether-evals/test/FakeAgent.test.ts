import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import type { AgentMessage } from "@aether-agent/sdk";
import { FakeAgent, Task, Transcript, Workspace } from "../src/index.js";

describe("FakeAgent", () => {
  it("success() ends with a done message", async () => {
    const result = await Transcript.fromStream(
      FakeAgent.success().run(new Task("t")),
    );

    expect(result.messages.map((message) => message.type)).toEqual([
      "text",
      "done",
    ]);
  });

  it("withToolCall() streams tool_result, text, then done", async () => {
    const result = await Transcript.fromStream(
      FakeAgent.withToolCall("bash", "ok").run(new Task("t")),
    );

    expect(result.messages.map((message) => message.type)).toEqual([
      "tool_result",
      "text",
      "done",
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
      seen.push(message.type);
      trace.add(message);
    }

    expect(seen).toEqual(["text", "done"]);
    expect(trace.messages.map((message) => message.type)).toEqual(seen);
  });

  it("context_usage messages in the transcript flow into usage", async () => {
    const usageMessage: AgentMessage = {
      type: "context_usage",
      input_tokens: 200,
      output_tokens: 20,
      cache_read_tokens: 50,
      cache_creation_tokens: 0,
      reasoning_tokens: 7,
      usage_ratio: 0.5,
      context_limit: 200_000,
      total_input_tokens: 3000,
      total_output_tokens: 600,
      total_cache_read_tokens: 50,
      total_cache_creation_tokens: 0,
      total_reasoning_tokens: 7,
    };
    const agent = new FakeAgent([
      {
        type: "text",
        message_id: "fake_1",
        chunk: "ok",
        is_complete: true,
        model_name: "fake",
      },
      usageMessage,
      { type: "done" },
    ]);

    const result = await Transcript.fromStream(agent.run(new Task("t")));

    expect(result.messages.map((message) => message.type)).toContain(
      "context_usage",
    );
    expect(result.usage().total_input_tokens).toBe(3000);
    expect(result.usage().total_output_tokens).toBe(600);
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
