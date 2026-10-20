import { describe, expect, it, vi } from "vitest";

import type { AgentEvent } from "@aether-agent/sdk";
import { DockerAgent, Task, Transcript, turnEnded } from "../src/index.js";
import { FakeRuntime } from "./FakeRuntime.js";

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
            server: null,
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
    { status: "failed", message_id: "error-1", error: "boom" } as const,
    { status: "cancelled" } as const,
  ])(
    "keeps the transcript through the first $status turn outcome",
    async (outcome) => {
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

      const transcript = await Transcript.fromStream(
        agent.run(new Task("test")),
      );

      expect(transcript.events).toEqual([ended]);
    },
  );

  it("gives the agent only its own environment and the eval variables", async () => {
    vi.stubEnv("ANTHROPIC_API_KEY", "host-key");
    vi.stubEnv("AETHER_LLM_MODEL", "host-model");
    try {
      const runtime = runtimeEmitting(JSON.stringify(turnEnded()));
      const agent = new DockerAgent({
        container: runtime.container(),
        command: ["fake-agent"],
        env: { MODEL: "eval-model" },
      });

      await collect(agent.run(new Task("do the thing")));

      expect(runtime.execs[0]?.env).toEqual({
        MODEL: "eval-model",
        AETHER_EVAL_WORKSPACE_ROOT: "/workspace",
        AETHER_EVAL_CWD: "/workspace",
        AETHER_EVAL_TASK_PROMPT: "do the thing",
      });
    } finally {
      vi.unstubAllEnvs();
    }
  });

  it("rejects a run that is aborted while the agent is working", async () => {
    const agent = new DockerAgent({
      container: new FakeRuntime({ hang: true }).container(),
      command: ["fake-agent"],
    });
    const controller = new AbortController();

    const run = collect(
      agent.run(new Task("test"), { signal: controller.signal }),
    );
    controller.abort();

    await expect(run).rejects.toMatchObject({ code: "aborted" });
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
  return new DockerAgent({
    container: runtimeEmitting(...lines).container(),
    command: ["fake-agent"],
  });
}

function runtimeEmitting(...lines: string[]): FakeRuntime {
  return new FakeRuntime({ stdout: lines.map((line) => `${line}\n`).join("") });
}

async function collect(
  stream: AsyncIterable<AgentEvent>,
): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  for await (const event of stream) events.push(event);
  return events;
}
