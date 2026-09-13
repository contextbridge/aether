import { fileURLToPath } from "node:url";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { describe, expect, it } from "vitest";
import { z } from "zod";

import {
  AetherSession,
  acp,
  mcp,
  type AetherMessage,
  tool,
} from "../src/index.js";
import { sessionUsageFactory } from "./factories/sessionUsage.js";
import { TRACE_CONTEXT } from "./traceContext.js";

const FAKE_AETHER = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeAether.mjs",
);

describe("AetherSession with a fake ACP agent", () => {
  it("negotiates v2 and completes only after the raw idle update", async () => {
    await using session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
    });
    expect(session.initializeResponse.protocolVersion).toBe(2);
    const messages = await Array.fromAsync(session.prompt("hello"));
    const updates = messages.filter((m) => m.type === "session_update");
    expect(updates.map((m) => m.update.sessionUpdate)).toEqual([
      "user_message",
      "state_update",
      "agent_message_chunk",
      "state_update",
    ]);
    expect(updates.at(-1)?.update).toMatchObject({
      sessionUpdate: "state_update",
      state: "idle",
      stopReason: "end_turn",
    });
    expect(messages.at(-1)).toEqual({
      type: "result",
      sessionId: session.sessionId,
      stopReason: "end_turn",
    });
  });

  it.each([
    { FAKE_AETHER_IDLE_BEFORE_ACK: "1" },
    { FAKE_AETHER_NO_STOP_REASON: "1" },
    {
      FAKE_AETHER_READY_IDLE: "1",
      FAKE_AETHER_UNRELATED_IDLE: "1",
      FAKE_AETHER_DUPLICATE_IDLE: "1",
    },
  ])("correlates completion across notification ordering: %j", async (env) => {
    await using session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, ...env },
    });
    for (const prompt of ["first", "second"]) {
      const messages = await Array.fromAsync(session.prompt(prompt));
      expect(messages.filter((m) => m.type === "result")).toEqual([
        {
          type: "result",
          sessionId: session.sessionId,
          stopReason: "end_turn",
        },
      ]);
      const chunk = messages.find(
        (m) =>
          m.type === "session_update" &&
          acp.SessionUpdate.isAgentMessageChunk(m.update),
      );
      expect(chunk).toBeDefined();
    }
  });

  it.each([{}, { FAKE_AETHER_IDLE_BEFORE_ACK: "1" }])(
    "keeps a prompt busy until cancellation reaches idle: %j",
    async (env) => {
      await using session = await AetherSession.start({
        binaryPath: FAKE_AETHER,
        env: {
          PATH: process.env.PATH,
          FAKE_AETHER_WAIT_FOR_CANCEL: "1",
          ...env,
        },
      });
      const messages: AetherMessage[] = [];
      for await (const message of session.prompt("wait")) {
        messages.push(message);
        if (
          message.type === "session_update" &&
          acp.SessionUpdate.isStateUpdate(message.update) &&
          acp.StateUpdate.isRunning(message.update)
        ) {
          expect(messages.some((m) => m.type === "result")).toBe(false);
          await expect(
            Array.fromAsync(session.prompt("overlap")),
          ).rejects.toMatchObject({ code: "prompt_in_progress" });
          await session.cancel();
        }
      }
      expect(messages.at(-1)).toEqual({
        type: "result",
        sessionId: session.sessionId,
        stopReason: "cancelled",
      });
    },
  );

  it("terminates post-acceptance failures with an error message and idle", async () => {
    await using session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_FAIL_AFTER_ACK: "1" },
    });
    const messages = await Array.fromAsync(session.prompt("fail"));
    expect(messages).toContainEqual(
      expect.objectContaining({
        type: "session_update",
        update: expect.objectContaining({
          content: { type: "text", text: "Error: Fake post-ack failure" },
        }),
      }),
    );
    expect(messages.at(-1)).toMatchObject({
      type: "result",
      stopReason: "end_turn",
    });
  });

  it("reports disconnection rather than silently ending an accepted turn", async () => {
    await using session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_DISCONNECT_AFTER_ACK: "1" },
    });
    await expect(
      Array.fromAsync(session.prompt("disconnect")),
    ).rejects.toMatchObject({ code: "process_exited" });
  });

  it("allows another prompt after a submission is rejected", async () => {
    await using session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
    });
    await expect(
      Array.fromAsync(session.prompt("reject-submission")),
    ).rejects.toBeDefined();
    const messages = await Array.fromAsync(session.prompt("retry"));
    expect(messages.at(-1)).toMatchObject({
      type: "result",
      stopReason: "end_turn",
    });
  });

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

  it("forwards provider URL, auth, and inference profile overrides to the CLI", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "aether-sdk-"));
    const logFile = path.join(dir, "fake-aether.log");
    const arn =
      "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/planner-profile";
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      providers: {
        bedrock: {
          url: "http://127.0.0.1:8787",
          auth: "none",
          inferenceProfileArn: arn,
        },
      },
      settings: { credentialsStore: { type: "memory" }, agents: [] },
      env: { PATH: process.env.PATH, FAKE_AETHER_LOG_FILE: logFile },
    });

    try {
      const lines = (await readFile(logFile, "utf8")).trim().split("\n");
      const argv = lines
        .map((line) => JSON.parse(line))
        .find((event) => event.event === "argv");
      expect(argv.args).toContain("--options-json");
      const optionsJson = argv.args[argv.args.indexOf("--options-json") + 1];
      expect(JSON.parse(optionsJson)).toEqual({
        providers: {
          bedrock: {
            url: "http://127.0.0.1:8787",
            auth: "none",
            inferenceProfileArn: arn,
          },
        },
        settings: {
          credentialsStore: { type: "memory" },
          agents: [],
        },
      });
    } finally {
      await session.close();
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("forwards trace context to the ACP process", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "aether-sdk-"));
    const logFile = path.join(dir, "fake-aether.log");
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      traceContext: TRACE_CONTEXT,
      env: { PATH: process.env.PATH, FAKE_AETHER_LOG_FILE: logFile },
    });

    try {
      const events = (await readFile(logFile, "utf8"))
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      const argv = events.find((event) => event.event === "argv");
      const optionsJson = argv.args[argv.args.indexOf("--options-json") + 1];
      expect(JSON.parse(optionsJson).traceContext).toEqual(TRACE_CONTEXT);
    } finally {
      await session.close();
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("accepts per-agent contextWindow in inline settings", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      agent: "planner",
      settings: {
        agents: [
          {
            name: "planner",
            description: "Planner agent",
            model: "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0",
            providers: {
              bedrock: {
                inferenceProfileArn:
                  "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/planner-profile",
              },
            },
            contextWindow: 200000,
            userInvocable: true,
            prompts: [{ type: "text", text: "Plan carefully." }],
          },
        ],
      },
    });

    try {
      expect(session.sessionId).toMatch(/^fake-session-/);
    } finally {
      await session.close();
    }
  });

  it("starts a session and handles native ACP elicitation", async () => {
    const requests: unknown[] = [];
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_REQUEST_ELICITATION: "1" },
      onElicitation: async (request) => {
        requests.push(request);
        return { action: "accept", content: { name: "Ada" } };
      },
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
    expect(types.slice(0, -1).sort()).toEqual([
      "elicitation_complete",
      "session_update",
      "session_update",
      "session_update",
      "session_update",
    ]);
    expect(types.at(-1)).toBe("result");
    expect(requests).toMatchObject([
      { mode: "form", message: "What is your name?" },
    ]);
    const result = messages.find((m) => m.type === "result");
    if (result?.type === "result") {
      expect(result.stopReason).toBe("end_turn");
    }
  });

  it("exposes session usage", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: {
        PATH: process.env.PATH,
        FAKE_AETHER_EXT_NOTIFICATION: JSON.stringify({
          method: "_aether/session_usage",
          params: { usage: sessionUsageFactory.build() },
        }),
      },
    });

    try {
      const messages: AetherMessage[] = [];
      for await (const message of session.prompt("test prompt")) {
        messages.push(message);
      }

      expect(
        messages
          .slice(0, -1)
          .map((message) => message.type)
          .sort(),
      ).toEqual([
        "session_update",
        "session_update",
        "session_update",
        "session_update",
        "usage",
      ]);
      expect(messages.at(-1)?.type).toBe("result");

      expect(messages.find((message) => message.type === "usage")).toEqual({
        type: "usage",
        usage: sessionUsageFactory.build(),
      });
    } finally {
      await session.close();
    }
  });

  it("ignores unknown ACP extension notifications", async () => {
    const notification = {
      method: "_example.com/status",
      params: { status: "ready" },
    };
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: {
        PATH: process.env.PATH,
        FAKE_AETHER_EXT_NOTIFICATION: JSON.stringify(notification),
      },
    });

    try {
      const messages: AetherMessage[] = [];
      for await (const message of session.prompt("test prompt")) {
        messages.push(message);
      }
      expect(messages.map((message) => message.type)).toEqual([
        "session_update",
        "session_update",
        "session_update",
        "session_update",
        "result",
      ]);
    } finally {
      await session.close();
    }
  });

  it("rejects malformed session usage notifications", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: {
        PATH: process.env.PATH,
        FAKE_AETHER_EXT_NOTIFICATION: JSON.stringify({
          method: "_aether/session_usage",
          params: {},
        }),
      },
    });

    try {
      await expect(async () => {
        for await (const _message of session.prompt("test prompt")) {
          void _message;
        }
      }).rejects.toMatchObject({ code: "invalid_protocol_message" });
    } finally {
      await session.close();
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

      expect(first.map((m) => m.type)).toEqual([
        "session_update",
        "session_update",
        "session_update",
        "session_update",
        "result",
      ]);
      expect(second.map((m) => m.type)).toEqual([
        "session_update",
        "session_update",
        "session_update",
        "session_update",
        "result",
      ]);
    } finally {
      await session.close();
    }
  });

  it("uses provided env instead of inheriting process.env", async () => {
    const originalExtraChunks = process.env.FAKE_AETHER_EXTRA_CHUNKS;
    process.env.FAKE_AETHER_EXTRA_CHUNKS = "1";

    let session: AetherSession | null = null;
    try {
      session = await AetherSession.start({
        binaryPath: FAKE_AETHER,
        env: { PATH: process.env.PATH },
      });

      const messages: AetherMessage[] = [];
      for await (const message of session.prompt("test prompt")) {
        messages.push(message);
      }

      const updateTexts = messages.flatMap((m) =>
        m.type === "session_update" &&
        acp.SessionUpdate.isAgentMessageChunk(m.update) &&
        acp.ContentBlock.isText(m.update.content)
          ? [m.update.content.text]
          : [],
      );
      expect(updateTexts).toEqual(["hello from fake aether"]);
    } finally {
      if (originalExtraChunks === undefined) {
        delete process.env.FAKE_AETHER_EXTRA_CHUNKS;
      } else {
        process.env.FAKE_AETHER_EXTRA_CHUNKS = originalExtraChunks;
      }
      await session?.close();
    }
  });

  it("does not surface stale events from an abandoned prompt on the next prompt", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_EXTRA_CHUNKS: "2" },
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
        acp.SessionUpdate.isAgentMessageChunk(m.update) &&
        acp.ContentBlock.isText(m.update.content)
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

  it("releases the turn when consumer breaks immediately after the result event", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
    });

    try {
      for await (const message of session.prompt("first")) {
        if (message.type === "result") break;
      }

      const second: AetherMessage[] = [];
      for await (const message of session.prompt("second")) {
        second.push(message);
      }
      expect(second.map((m) => m.type)).toEqual([
        "session_update",
        "session_update",
        "session_update",
        "session_update",
        "result",
      ]);
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

    await using custom = await mcp({ name: "custom", tools: [submit] });
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: {
        PATH: process.env.PATH,
        FAKE_AETHER_CALL_MCP_SERVER: "custom",
        FAKE_AETHER_TOOL: "submit",
        FAKE_AETHER_TOOL_ARGS: JSON.stringify({ answer: "42" }),
      },
      settings: { agents: [], mcps: [custom.spec] },
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

  it("forwards per-agent mcps to the spawned agent and scopes each source", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "aether-sdk-per-agent-"));
    const logFile = path.join(dir, "fake-aether.log");
    await using planner = await mcp({
      name: "planner-tools",
      tools: [
        tool({
          name: "plan",
          description: "plan",
          inputSchema: {},
          handler: async () => ({ content: [] }),
        }),
      ],
    });

    await using reviewer = await mcp({
      name: "reviewer-tools",
      tools: [
        tool({
          name: "review",
          description: "review",
          inputSchema: {},
          handler: async () => ({ content: [] }),
        }),
      ],
    });

    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_LOG_FILE: logFile },
      settings: {
        agents: [
          {
            name: "planner",
            description: "Planner",
            model: "anthropic:claude-sonnet-4-5",
            userInvocable: true,
            mcps: [planner.spec],
          },
          {
            name: "reviewer",
            description: "Reviewer",
            model: "anthropic:claude-sonnet-4-5",
            userInvocable: true,
            mcps: [reviewer.spec],
          },
        ],
      },
    });

    try {
      const lines = (await readFile(logFile, "utf8")).trim().split("\n");
      const argv = lines
        .map((line) => JSON.parse(line))
        .find((event) => event.event === "argv");
      const optionsJson = argv.args[argv.args.indexOf("--options-json") + 1];
      const options = JSON.parse(optionsJson);
      const agents = options.settings.agents;

      const plannerServer = agents[0].mcps[0].servers["planner-tools"];
      expect(plannerServer).toMatchObject({ type: "http" });
      expect(agents[0].mcps).toHaveLength(1);

      const reviewerServer = agents[1].mcps[0].servers["reviewer-tools"];
      expect(reviewerServer).toMatchObject({ type: "http" });
      expect(agents[1].mcps).toHaveLength(1);

      expect(agents[0].mcps[0]).not.toBe(agents[1].mcps[0]);
    } finally {
      await session.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
});
