import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import type { AgentMessage } from "@aether-agent/sdk";
import { AetherEvalError } from "./errors.js";
import type { Agent } from "./Agent.js";
import type { Task } from "./task.js";
import type { Workspace } from "./workspace.js";

/**
 * In-memory {@link Agent} for testing eval harnesses without Docker. Streams a scripted transcript
 * and optionally writes files into the workspace, mirroring how a real agent is observed.
 */
export class FakeAgent implements Agent {
  constructor(
    readonly messages: AgentMessage[],
    readonly fileWrites: ReadonlyArray<readonly [string, string]> = [],
    readonly workspace: Workspace | undefined = undefined,
  ) {}

  static success(): FakeAgent {
    return new FakeAgent([
      {
        type: "text",
        message_id: "fake_1",
        chunk: "Task completed successfully",
        is_complete: true,
        model_name: "fake",
      },
      { type: "done" },
    ]);
  }

  static withToolCall(toolName: string, result: string): FakeAgent {
    return new FakeAgent([
      {
        type: "tool_result",
        model_name: "fake",
        result: { id: "fake_call_1", name: toolName, arguments: "{}", result },
      },
      {
        type: "text",
        message_id: "fake_2",
        chunk: "Task completed using tools",
        is_complete: true,
        model_name: "fake",
      },
      { type: "done" },
    ]);
  }

  static writesFile(path: string, contents: string): FakeAgent {
    return FakeAgent.success().withFileWrite(path, contents);
  }

  withFileWrite(path: string, contents: string): FakeAgent {
    return new FakeAgent(
      this.messages,
      [...this.fileWrites, [path, contents]],
      this.workspace,
    );
  }

  withWorkspace(workspace: Workspace): FakeAgent {
    return new FakeAgent(this.messages, this.fileWrites, workspace);
  }

  async *run(_task: Task): AsyncIterable<AgentMessage> {
    if (this.fileWrites.length > 0 && !this.workspace) {
      throw new AetherEvalError(
        "configuration_error",
        "FakeAgent file writes require a bound workspace",
      );
    }

    for (const [relativePath, contents] of this.fileWrites) {
      const target = join(this.workspace!.path, relativePath);
      await mkdir(dirname(target), { recursive: true });
      await writeFile(target, contents);
    }

    for (const message of this.messages) {
      yield message;
    }
  }
}
