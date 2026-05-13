#!/usr/bin/env node
// Tiny fake "aether acp" stand-in. Speaks ACP over stdio.
//
// Behavior:
//   - initialize -> respond with V1 capabilities.
//   - newSession -> echo session id, store mcpServers + _meta to log file.
//   - prompt -> emit a session_update chunk, optionally request a permission
//     decision and/or call a custom MCP tool, then return stopReason="end_turn".
//
// Configurable via env:
//   FAKE_AETHER_CALL_MCP_SERVER   Name of the SDK-supplied MCP server to call
//   FAKE_AETHER_TOOL              Tool name to call (default "submit")
//   FAKE_AETHER_TOOL_ARGS         JSON-encoded args (default {"value":"hi"})
//   FAKE_AETHER_REQUEST_PERMISSION  If set, send a requestPermission RPC and
//                                 echo the chosen outcome as the chunk text.
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

const writable = Writable.toWeb(process.stdout);
const readable = Readable.toWeb(process.stdin);
const stream = ndJsonStream(writable, readable);

log(JSON.stringify({ event: "argv", args: process.argv.slice(2) }));

let capturedSessionId = null;
let capturedMcpServers = [];
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
    capturedMcpServers = params.mcpServers ?? [];
    capturedMeta = params._meta ?? null;
    log(
      JSON.stringify({
        event: "newSession",
        mcpServers: capturedMcpServers,
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

    const callName = process.env.FAKE_AETHER_CALL_MCP_SERVER;
    if (callName) {
      const server = capturedMcpServers.find((s) => s.name === callName);
      if (!server || server.type !== "http") {
        throw new Error(
          `Fake agent could not find http MCP server named ${callName}`,
        );
      }
      const headers = Object.fromEntries(
        server.headers.map((h) => [h.name, h.value]),
      );

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
