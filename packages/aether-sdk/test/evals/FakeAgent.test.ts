import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { FakeAgent, Workspace } from "../../src/evals/index.js";

describe("FakeAgent", () => {
  it("success() ends with a done message", async () => {
    const result = await FakeAgent.success().run({
      workspace: await workspace(),
      taskPrompt: "t",
    });

    expect(result.transcript.map((message) => message.type)).toEqual([
      "text",
      "done",
    ]);
  });

  it("withToolCall() streams tool_result, text, then done", async () => {
    const result = await FakeAgent.withToolCall("bash", "ok").run({
      workspace: await workspace(),
      taskPrompt: "t",
    });

    expect(result.transcript.map((message) => message.type)).toEqual([
      "tool_result",
      "text",
      "done",
    ]);
  });

  it("writesFile() writes into the workspace, including nested paths", async () => {
    const ws = await workspace();

    await FakeAgent.writesFile("nested/hello.txt", "hello").run({
      workspace: ws,
      taskPrompt: "t",
    });

    expect(await readFile(join(ws.path, "nested/hello.txt"), "utf8")).toBe(
      "hello",
    );
  });

  it("forwards each message to onMessage", async () => {
    const seen: string[] = [];

    await FakeAgent.success().run(
      { workspace: await workspace(), taskPrompt: "t" },
      { onMessage: (message) => seen.push(message.type) },
    );

    expect(seen).toEqual(["text", "done"]);
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
