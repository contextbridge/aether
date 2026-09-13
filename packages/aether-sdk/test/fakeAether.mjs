#!/usr/bin/env node
// Tiny fake "aether acp" stand-in. Speaks ACP over stdio.
//
// Behavior:
//   - initialize -> respond with V2 capabilities.
//   - newSession -> echo session id, store settings + _meta to log file.
//   - prompt -> emit a session_update chunk, optionally request a permission
//     decision, request an elicitation, and/or call a custom MCP tool, then
//     emit idle with stopReason="end_turn" after the acceptance ack.
//
// Configurable via env:
//   FAKE_AETHER_CALL_MCP_SERVER   Name of the SDK-supplied MCP server to call
//   FAKE_AETHER_TOOL              Tool name to call (default "submit")
//   FAKE_AETHER_TOOL_ARGS         JSON-encoded args (default {"value":"hi"})
//   FAKE_AETHER_REQUEST_PERMISSION  If set, send a requestPermission RPC and
//                                 echo the chosen outcome as the chunk text.
//   FAKE_AETHER_REQUEST_ELICITATION  If set, exercise ACP elicitation.
//   FAKE_AETHER_LOG_FILE          Optional path; debug events written there.

import { Readable, Writable } from "node:stream";
import {
  agent as createAgent,
  ndJsonStream,
  batchNotification,
} from "@agentclientprotocol/sdk/experimental/v2";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { appendFileSync } from "node:fs";

const log = (line) => {
  if (process.env.FAKE_AETHER_LOG_FILE) {
    appendFileSync(process.env.FAKE_AETHER_LOG_FILE, line + "\n");
  }
};

if (process.argv[2] === "headless") {
  const args = process.argv.slice(2);
  log(JSON.stringify({ event: "headless", args }));
  const optionsIndex = args.indexOf("--options-json");
  const options =
    optionsIndex >= 0 ? JSON.parse(args[optionsIndex + 1] ?? "{}") : {};
  const { writeFakeOutput } = await import("./fakeCommand.mjs");
  const events = [
    {
      category: "message",
      event: {
        type: "text",
        message_id: "text",
        chunk: options.prompt,
        is_complete: true,
      },
    },
    {
      category: "turn",
      event: { type: "ended", outcome: { status: "completed" } },
    },
  ];
  await writeFakeOutput(
    process.env.FAKE_STDOUT ?? events.map((e) => JSON.stringify(e)).join("\n"),
  );
}

const writable = Writable.toWeb(process.stdout);
const readable = Readable.toWeb(process.stdin);
const stream = ndJsonStream(writable, readable);

const argv = process.argv.slice(2);
log(JSON.stringify({ event: "argv", args: argv }));

const argvOptionsIndex = argv.indexOf("--options-json");
const argvOptions =
  argvOptionsIndex >= 0 ? JSON.parse(argv[argvOptionsIndex + 1] ?? "{}") : {};
const settings = argvOptions.settings ?? {};

function collectInlineServers() {
  const map = new Map();
  const ingest = (source) => {
    if (typeof source !== "object" || source === null) return;
    if (source.type !== "inline") return;
    for (const [name, config] of Object.entries(source.servers ?? {})) {
      map.set(name, config);
    }
  };
  for (const source of settings.mcps ?? []) ingest(source);
  for (const agent of settings.agents ?? []) {
    for (const source of agent.mcps ?? []) ingest(source);
  }
  return map;
}
const inlineServers = collectInlineServers();

let capturedSessionId = null;
let capturedMeta = null;
let conn;
let activeTurn;
let messageSequence = 0;

const agent = {
  async initialize(params) {
    log(JSON.stringify({ event: "initialize", params }));
    return {
      protocolVersion: 2,
      info: { name: "fake-aether", version: "0.0.1" },
      capabilities: { session: { mcp: { http: {}, stdio: {} } } },
      authMethods: [],
    };
  },

  async newSession(params) {
    capturedSessionId =
      "fake-session-" + Math.random().toString(36).slice(2, 8);
    capturedMeta = params._meta ?? null;
    log(
      JSON.stringify({
        event: "newSession",
        settings,
        meta: capturedMeta,
      }),
    );
    if (process.env.FAKE_AETHER_READY_IDLE) await sendIdle(capturedSessionId);
    return { sessionId: capturedSessionId, configOptions: [] };
  },

  async prompt(params) {
    if (activeTurn) throw new Error("Busy");
    if (params.prompt[0]?.text === "reject-submission")
      throw new Error("Rejected submission");
    const turn = {
      sessionId: params.sessionId,
      cancelled: false,
      messageId: `message-${++messageSequence}`,
    };
    activeTurn = turn;
    if (process.env.FAKE_AETHER_IDLE_BEFORE_ACK) {
      await runTurn(params, turn);
    } else {
      setImmediate(() => void runTurn(params, turn));
    }
    return {};
  },
  async cancel() {
    if (activeTurn) {
      activeTurn.cancelled = true;
      activeTurn.release?.();
    }
  },
  async login() {
    return {};
  },
  async logout() {
    return {};
  },
  async setSessionConfigOption() {
    return { configOptions: [] };
  },
  async listSessions() {
    return { sessions: [] };
  },
  async resumeSession(params) {
    if (params.replayFrom?.type === "start") {
      await notifyUpdate(params.sessionId, {
        sessionUpdate: "user_message",
        messageId: "history-user",
        content: [{ type: "text", text: "history" }],
      });
      await sendIdle(params.sessionId);
    }
    return { configOptions: [] };
  },
  async closeSession() {
    return {};
  },
};

async function notifyUpdate(sessionId, update) {
  await conn.notify("session/update", { sessionId, update });
}

async function sendIdle(sessionId, stopReason) {
  await notifyUpdate(sessionId, {
    sessionUpdate: "state_update",
    state: "idle",
    ...(stopReason ? { stopReason } : {}),
  });
}

async function runTurn(params, turn) {
  try {
    await notifyUpdate(params.sessionId, {
      sessionUpdate: "user_message",
      messageId: `user-${turn.messageId}`,
      content: params.prompt,
    });
    await notifyUpdate(params.sessionId, {
      sessionUpdate: "state_update",
      state: "running",
    });
    if (process.env.FAKE_AETHER_UNRELATED_IDLE)
      await sendIdle("unrelated-session", "cancelled");
    if (process.env.FAKE_AETHER_WAIT_FOR_CANCEL)
      await new Promise((resolve) => {
        turn.release = resolve;
        if (turn.cancelled) resolve();
      });
    if (process.env.FAKE_AETHER_DISCONNECT_AFTER_ACK) process.exit(0);
    if (process.env.FAKE_AETHER_FAIL_AFTER_ACK)
      throw new Error("Fake post-ack failure");
    log(
      JSON.stringify({
        event: "prompt",
        sessionId: params.sessionId,
        prompt: params.prompt,
      }),
    );

    let chunkText = "hello from fake aether";
    if (process.env.FAKE_AETHER_REQUEST_PERMISSION) {
      const decision = await conn.request("session/request_permission", {
        sessionId: params.sessionId,
        title: "Run test tool?",
        subject: {
          type: "tool_call",
          toolCall: {
            toolCallId: "tc-1",
            title: "test",
            kind: "execute",
            rawInput: {},
          },
        },
        options: [
          { optionId: "allow", name: "Allow", kind: "allow_once" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      });
      chunkText = JSON.stringify(decision.outcome);
    }

    if (process.env.FAKE_AETHER_REQUEST_ELICITATION) {
      const response = await conn.request("elicitation/create", {
        mode: "form",
        sessionId: params.sessionId,
        requestedSchema: {
          type: "object",
          properties: { name: { type: "string", title: "Name" } },
        },
        message: "What is your name?",
      });
      chunkText = JSON.stringify(response);
      await conn.notify("elicitation/complete", { elicitationId: "elicit-1" });
    }

    await notifyUpdate(params.sessionId, {
      sessionUpdate: "agent_message_chunk",
      messageId: turn.messageId,
      content: { type: "text", text: chunkText },
    });

    const extraChunks = Number(process.env.FAKE_AETHER_EXTRA_CHUNKS ?? "0");
    for (let i = 0; i < extraChunks; i++) {
      await notifyUpdate(params.sessionId, {
        sessionUpdate: "agent_message_chunk",
        messageId: turn.messageId,
        content: { type: "text", text: `chunk-${i + 2}` },
      });
    }

    if (process.env.FAKE_AETHER_EXT_NOTIFICATION) {
      const notification = JSON.parse(process.env.FAKE_AETHER_EXT_NOTIFICATION);
      await conn.notify(notification.method, notification.params);
    }

    const callName = process.env.FAKE_AETHER_CALL_MCP_SERVER;
    if (callName) {
      const server = inlineServers.get(callName);
      if (!server || server.type !== "http") {
        throw new Error(
          `Fake agent could not find inline http MCP server named ${callName}`,
        );
      }
      const headers = server.headers ?? {};

      const transport = new StreamableHTTPClientTransport(new URL(server.url), {
        requestInit: { headers },
      });

      const client = new Client({ name: "fake-aether", version: "0.0.1" });
      await client.connect(transport);
      try {
        const toolName = process.env.FAKE_AETHER_TOOL ?? "submit";
        const toolArgs = JSON.parse(
          process.env.FAKE_AETHER_TOOL_ARGS ?? '{"value":"hi"}',
        );

        const result = await client.callTool({
          name: toolName,
          arguments: toolArgs,
        });

        log(JSON.stringify({ event: "tool_result", result }));
      } finally {
        await client.close();
      }
    }
  } catch (error) {
    await notifyUpdate(params.sessionId, {
      sessionUpdate: "agent_message_chunk",
      messageId: turn.messageId,
      content: { type: "text", text: String(error) },
    });
  } finally {
    activeTurn = null;
    const stopReason = turn.cancelled
      ? "cancelled"
      : process.env.FAKE_AETHER_NO_STOP_REASON
        ? undefined
        : "end_turn";
    if (process.env.FAKE_AETHER_DUPLICATE_IDLE) {
      await conn.batch(
        [stopReason, "cancelled"].map((reason) =>
          batchNotification("session/update", {
            sessionId: params.sessionId,
            update: {
              sessionUpdate: "state_update",
              state: "idle",
              stopReason: reason,
            },
          }),
        ),
      );
    } else {
      await sendIdle(params.sessionId, stopReason);
    }
  }
}

const app = createAgent()
  .onRequest("initialize", ({ params }) => agent.initialize(params))
  .onRequest("session/new", ({ params }) => agent.newSession(params))
  .onRequest("session/prompt", ({ params }) => agent.prompt(params))
  .onNotification("session/cancel", () => agent.cancel())
  .onRequest("auth/login", () => agent.login())
  .onRequest("auth/logout", () => agent.logout())
  .onRequest("session/resume", ({ params }) => agent.resumeSession(params))
  .onRequest("session/list", () => agent.listSessions())
  .onRequest("session/close", () => agent.closeSession())
  .onRequest("session/set_config_option", () => agent.setSessionConfigOption());
conn = app.connect(stream).client;
