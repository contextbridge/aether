import { fileURLToPath } from "node:url";
import path from "node:path";

import { describe, expect, it } from "vitest";
import { z } from "zod";

import { AetherSession, type AetherMessage, tool } from "../src/index.js";

const FAKE_AETHER = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeAether.mjs",
);

describe("AetherSession with a fake ACP agent", () => {
  it("rejects mutually exclusive settings sources", async () => {
    await expect(
      AetherSession.start({
        binaryPath: FAKE_AETHER,
        settings: {
          agents: [
            {
              name: "default",
              description: "Default agent",
              model: "anthropic:claude-sonnet-4-5",
              userInvocable: true,
              prompts: [{ type: "text", text: "Be helpful" }],
            },
          ],
        },
        settingsFile: ".aether/settings.json",
      } as never),
    ).rejects.toMatchObject({ code: "invalid_options" });
  });

  it("rejects agent and model together at runtime", async () => {
    await expect(
      AetherSession.start({
        binaryPath: FAKE_AETHER,
        agent: "planner",
        model: "anthropic:claude-sonnet-4-5",
      } as never),
    ).rejects.toMatchObject({ code: "invalid_options" });
  });

  it("starts an explicit session and streams a final result", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
    });

    const messages: AetherMessage[] = [];
    try {
      for await (const message of session.prompt("test prompt")) {
        messages.push(message);
      }
    } finally {
      await session.close();
    }

    expect(session.sessionId).toMatch(/^fake-session-/);
    const types = messages.map((m) => m.type);
    expect(types).toEqual(["session_update", "result"]);
    const result = messages.find((m) => m.type === "result");
    if (result?.type === "result") {
      expect(result.stopReason).toBe("end_turn");
    }
  });

  it("supports multiple prompts on the same session", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
    });

    try {
      const first: AetherMessage[] = [];
      for await (const message of session.prompt("first")) first.push(message);

      const second: AetherMessage[] = [];
      for await (const message of session.prompt("second"))
        second.push(message);

      expect(first.map((m) => m.type)).toEqual(["session_update", "result"]);
      expect(second.map((m) => m.type)).toEqual(["session_update", "result"]);
    } finally {
      await session.close();
    }
  });

  it("does not surface stale events from an abandoned prompt on the next prompt", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { FAKE_AETHER_EXTRA_CHUNKS: "2" },
    });

    try {
      // Break after the first chunk; later chunks + result for prompt 1 will
      // still arrive on the shared queue. The next prompt must not see them.
      for await (const _ of session.prompt("first")) {
        void _;
        break;
      }

      const second: AetherMessage[] = [];
      for await (const message of session.prompt("second")) {
        second.push(message);
      }

      const updateTexts = second.flatMap((m) =>
        m.type === "session_update" &&
        m.update.sessionUpdate === "agent_message_chunk" &&
        m.update.content.type === "text"
          ? [m.update.content.text]
          : [],
      );
      // The fake emits "hello from fake aether" + "chunk-2" + "chunk-3" per
      // prompt; the second prompt must see exactly its own three chunks, not
      // leftovers from the first.
      expect(updateTexts).toEqual([
        "hello from fake aether",
        "chunk-2",
        "chunk-3",
      ]);
      const results = second.filter((m) => m.type === "result");
      expect(results).toHaveLength(1);
    } finally {
      await session.close();
    }
  });

  it("bridges a closure-backed SDK MCP tool through to the fake agent", async () => {
    let received: string | null = null;
    const submit = tool({
      name: "submit",
      description: "submit",
      inputSchema: { answer: z.string() },
      handler: async ({ answer }) => {
        received = answer;
        return { content: [{ type: "text", text: "ok" }] };
      },
    });
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: {
        FAKE_AETHER_CALL_MCP_SERVER: "custom",
        FAKE_AETHER_TOOL: "submit",
        FAKE_AETHER_TOOL_ARGS: JSON.stringify({ answer: "42" }),
      },
      tools: { custom: [submit] },
    });

    try {
      for await (const _ of session.prompt("please call submit")) {
        void _;
      }
    } finally {
      await session.close();
    }

    expect(received).toBe("42");
  });
});
