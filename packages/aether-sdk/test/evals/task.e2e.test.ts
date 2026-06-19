import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  DockerAgent,
  DockerImage,
  Task,
  Workspace,
} from "../../src/evals/index.js";
import type { AgentMessage } from "../../src/generated/eval-types.js";

describe.skipIf(!process.env.AETHER_SDK_E2E)(
  "Task.run (e2e, requires Docker)",
  () => {
    it("runs an agent in a real container and returns its result", async () => {
      const seen: AgentMessage[] = [];
      const task = new Task(
        "write out.txt",
        await Workspace.fromFiles({ "notes.txt": "seed\n" }),
      );
      const agent = new DockerAgent({
        image: DockerImage.parse("alpine:3"),
        command: ["/bin/sh", "-c", script],
      });
      await using result = await task.run(agent, {
        onMessage: (message) => seen.push(message),
      });

      expect(result.passed).toBe(true);
      expect(result.toolCalls).toEqual([
        { name: "write", arguments: {}, rawArguments: "{}" },
      ]);
      expect(seen.map((message) => message.type)).toEqual([
        "tool_result",
        "done",
      ]);
      expect(
        await readFile(join(result.workspace.path, "out.txt"), "utf8"),
      ).toBe("modified\n");
    }, 120_000);
  },
);

const toolResult: AgentMessage = {
  type: "tool_result",
  model_name: "e2e",
  result: { id: "1", name: "write", arguments: "{}", result: "ok" },
};
const done: AgentMessage = { type: "done" };

const script = [
  'echo modified > "$AETHER_EVAL_CWD/out.txt"',
  `printf '%s\\n%s\\n' '${JSON.stringify(toolResult)}' '${JSON.stringify(done)}'`,
].join("; ");
