import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import type { AgentMessage, ContextUsage } from "../generated/eval-types.js";
import type {
  Agent,
  AgentConfig,
  AgentRunOptions,
  AgentRunResult,
} from "./Agent.js";

/**
 * In-memory {@link Agent} for testing eval harnesses without Docker. Streams a scripted transcript
 * and optionally writes files into the workspace, mirroring how a real agent is observed.
 */
export class FakeAgent implements Agent {
  readonly messages: AgentMessage[];
  readonly fileWrites: ReadonlyArray<readonly [string, string]>;

  constructor(
    messages: AgentMessage[],
    fileWrites: ReadonlyArray<readonly [string, string]> = [],
  ) {
    this.messages = messages;
    this.fileWrites = fileWrites;
  }

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

  /** Append a `context_usage` message reflecting `usage` to the scripted transcript. */
  withUsage(usage: ContextUsage): FakeAgent {
    return new FakeAgent(
      [
        ...this.messages,
        {
          type: "context_usage",
          ...usage,
        },
      ],
      this.fileWrites,
    );
  }

  withFileWrite(path: string, contents: string): FakeAgent {
    return new FakeAgent(this.messages, [...this.fileWrites, [path, contents]]);
  }

  async run(
    config: AgentConfig,
    options: AgentRunOptions = {},
  ): Promise<AgentRunResult> {
    for (const [relativePath, contents] of this.fileWrites) {
      const target = join(config.workspace.path, relativePath);
      await mkdir(dirname(target), { recursive: true });
      await writeFile(target, contents);
    }
    for (const message of this.messages) options.onMessage?.(message);
    return { transcript: this.messages, stderr: "" };
  }
}
