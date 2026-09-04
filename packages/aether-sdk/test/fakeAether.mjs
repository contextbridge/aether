#!/usr/bin/env node
// Tiny fake "aether acp" stand-in. Speaks ACP over stdio.
//
// Behavior:
//   - initialize -> respond with V1 capabilities.
//   - newSession -> echo session id, store settings + _meta to log file.
//   - prompt -> emit a session_update chunk, optionally request a permission
//     decision, request an elicitation, and/or call a custom MCP tool, then
//     return stopReason="end_turn".
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
import { AgentSideConnection, ndJsonStream } from "@agentclientprotocol/sdk";
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
  const prompt = options.prompt ?? args.at(-1);
  const output = options.output ?? args[args.indexOf("--output") + 1];
  if (process.env.FAKE_AETHER_HEADLESS_EXIT_CODE) {
    console.error("fake headless failed");
    process.exit(Number(process.env.FAKE_AETHER_HEADLESS_EXIT_CODE));
  }
  if (output === "json") {
    console.log(JSON.stringify({ type: "Text", chunk: prompt }));
    console.log(JSON.stringify({ type: "Done" }));
  } else {
    console.log(`fake headless: ${prompt}`);
  }
  process.exit(0);
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

const agent = {
  async initialize() {
    return {
      protocolVersion: 1,
      agentCapabilities: {
        loadSession: true,
        mcpCapabilities: { http: true, sse: true },
      },
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
    return { sessionId: capturedSessionId };
  },

  async prompt(params) {
    log(
      JSON.stringify({
        event: "prompt",
        sessionId: params.sessionId,
        prompt: params.prompt,
      }),
    );

    let chunkText = "hello from fake aether";
    if (process.env.FAKE_AETHER_REQUEST_PERMISSION) {
      const decision = await conn.requestPermission({
        sessionId: params.sessionId,
        toolCall: {
          toolCallId: "tc-1",
          title: "test",
          kind: "execute",
          rawInput: {},
        },
        options: [
          { optionId: "allow", name: "Allow", kind: "allow_once" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      });
      chunkText = JSON.stringify(decision.outcome);
    }

    if (process.env.FAKE_AETHER_REQUEST_ELICITATION) {
      const response = await conn.unstable_createElicitation({
        mode: "form",
        sessionId: params.sessionId,
        requestedSchema: {
          type: "object",
          properties: { name: { type: "string", title: "Name" } },
        },
        message: "What is your name?",
      });
      chunkText = JSON.stringify(response);
      await conn.unstable_completeElicitation({ elicitationId: "elicit-1" });
    }

    await conn.sessionUpdate({
      sessionId: params.sessionId,
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: chunkText },
      },
    });

    const extraChunks = Number(process.env.FAKE_AETHER_EXTRA_CHUNKS ?? "0");
    for (let i = 0; i < extraChunks; i++) {
      await conn.sessionUpdate({
        sessionId: params.sessionId,
        update: {
          sessionUpdate: "agent_message_chunk",
          content: { type: "text", text: `chunk-${i + 2}` },
        },
      });
    }

    if (process.env.FAKE_AETHER_EXT_NOTIFICATION) {
      const notification = JSON.parse(process.env.FAKE_AETHER_EXT_NOTIFICATION);
      await conn.extNotification(notification.method, notification.params);
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

    return { stopReason: "end_turn" };
  },
  async cancel() {
    /* no-op */
  },
  async authenticate() {
    return {};
  },
  async setSessionMode() {
    return {};
  },
  async setSessionConfigOption() {
    return { configOptions: [] };
  },
  async loadSession() {
    return {};
  },
  async listSessions() {
    return { sessions: [] };
  },
  async forkSession() {
    return {};
  },
  async resumeSession() {
    return {};
  },
  async closeSession() {
    return {};
  },
  async setSessionModel() {
    return {};
  },
  async listProviders() {
    return { providers: [] };
  },
  async setProviders() {
    return {};
  },
  async disableProviders() {
    return {};
  },
  async logout() {
    return {};
  },
};

conn = new AgentSideConnection(() => agent, stream);
