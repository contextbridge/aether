import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  Container,
  DockerAgent,
  Image,
  Task,
  ToolCall,
  Transcript,
  turnEnded,
  Workspace,
} from "../src/index.js";
import { eventName, logMessage } from "./logMessage.js";
import type { AgentEvent } from "@aether-agent/sdk";

describe.skipIf(!process.env.AETHER_EVALS_E2E)(
  "Transcript Docker collection (e2e, requires Docker)",
  () => {
    it("runs an agent in a real container and returns its result", async () => {
      const seen: AgentEvent[] = [];
      await using workspace = await Workspace.fromFiles({
        "notes.txt": "seed\n",
      });
      await using container = await Container.builder(
        Image.parse("alpine:3"),
      ).start(workspace);
      const agent = new DockerAgent({
        container,
        command: ["/bin/sh", "-c", script],
      });
      const trace = new Transcript();
      for await (const message of agent.run(new Task("write out.txt"))) {
        seen.push(message);
        logMessage(message);
        trace.add(message);
      }

      expect(trace.events.at(-1)).toMatchObject({
        category: "turn",
        event: { type: "ended" },
      });
      expect(trace.allToolCalls()).toEqual([new ToolCall("write", "{}")]);
      expect(seen.map(eventName)).toEqual(["tool:result", "turn:ended"]);
      expect(await readFile(join(workspace.path, "out.txt"), "utf8")).toBe(
        "modified\n",
      );
    }, 120_000);

    it("lets callers exec follow-up commands in the same container", async () => {
      await using workspace = await Workspace.empty();
      await using container = await Container.builder(
        Image.parse("alpine:3"),
      ).start(workspace);
      const agent = new DockerAgent({
        container,
        command: [
          "/bin/sh",
          "-c",
          `echo same-container > /tmp/aether-marker; printf '%s\n' '${JSON.stringify(done)}'`,
        ],
      });

      const trace = await Transcript.fromStream(
        agent.run(new Task("create marker")),
      );
      const output = await container.exec({
        command: ["/bin/sh", "-c", "cat /tmp/aether-marker"],
      });

      expect(trace.events.at(-1)).toMatchObject({
        category: "turn",
        event: { type: "ended" },
      });
      expect(output.exitCode).toBe(0);
      expect(output.stdout).toContain("same-container");
    }, 120_000);

    it("returns non-zero follow-up command exit codes", async () => {
      await using workspace = await Workspace.empty();
      await using container = await Container.builder(
        Image.parse("alpine:3"),
      ).start(workspace);

      const output = await container.exec({
        command: ["/bin/sh", "-c", "echo nope >&2; exit 7"],
      });

      expect(output.exitCode).toBe(7);
      expect(output.stderr).toContain("nope");
    }, 120_000);
  },
);

const toolResult: AgentEvent = {
  category: "tool",
  event: {
    type: "result",
    result: { id: "1", name: "write", arguments: "{}", result: "ok" },
  },
};
const done: AgentEvent = turnEnded();

const script = [
  'echo modified > "$AETHER_EVAL_CWD/out.txt"',
  `printf '%s\\n%s\\n' '${JSON.stringify(toolResult)}' '${JSON.stringify(done)}'`,
].join("; ");
