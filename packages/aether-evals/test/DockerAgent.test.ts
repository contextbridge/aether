import { describe, expect, it } from "vitest";

import type { AgentEvent } from "@aether-agent/sdk";
import { DockerAgent, Task, Transcript } from "../src/index.js";
import type { Container, ContainerStreamingOptions } from "../src/index.js";

describe("DockerAgent", () => {
  it("streams canonical events including tool definition updates", async () => {
    const definitionsUpdated: AgentEvent = {
      category: "tool",
      event: {
        type: "definitions_updated",
        tools: [
          {
            name: "weather",
            description: "Get the weather",
            parameters: { type: "object" },
          },
        ],
      },
    };
    const ended: AgentEvent = {
      category: "turn",
      event: { type: "ended", outcome: { status: "completed" } },
    };
    const agent = agentEmitting(definitionsUpdated, ended);

    const transcript = await Transcript.fromStream(agent.run(new Task("test")));

    expect(transcript.events).toEqual([definitionsUpdated, ended]);
  });

  it.each([
    { status: "completed" } as const,
    { status: "failed", error: "boom" } as const,
    { status: "cancelled" } as const,
  ])("stops at a $status turn outcome", async (outcome) => {
    const ended: AgentEvent = {
      category: "turn",
      event: { type: "ended", outcome },
    };
    const trailing: AgentEvent = {
      category: "message",
      event: {
        type: "text",
        chunk: "not emitted",
        is_complete: true,
        message_id: "message-1",
      },
    };
    const agent = agentEmitting(ended, trailing);

    const transcript = await Transcript.fromStream(agent.run(new Task("test")));

    expect(transcript.events).toEqual([ended]);
  });

  it("rejects malformed NDJSON", async () => {
    const agent = agentEmittingLines("not JSON");

    await expect(collect(agent.run(new Task("test")))).rejects.toMatchObject({
      name: "AetherEvalError",
      code: "agent_event_json_line",
    });
  });
});

function agentEmitting(...events: AgentEvent[]): DockerAgent {
  return agentEmittingLines(...events.map((event) => JSON.stringify(event)));
}

function agentEmittingLines(...lines: string[]): DockerAgent {
  const container = {
    workspaceRoot: "/workspace",
    cwd: "/workspace",
    async *execStreaming(_options: ContainerStreamingOptions) {
      yield* lines;
    },
  } as unknown as Container;
  return new DockerAgent({ container, command: ["fake-agent"] });
}

async function collect(
  stream: AsyncIterable<AgentEvent>,
): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  for await (const event of stream) events.push(event);
  return events;
}
