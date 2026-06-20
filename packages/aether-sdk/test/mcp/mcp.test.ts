import { once } from "node:events";
import { connect, type Socket } from "node:net";

import { describe, expect, it } from "vitest";
import { z } from "zod";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

import { mcp } from "../../src/mcp/index.js";
import type { InlineMcpSource } from "../../src/mcp/index.js";
import { tool } from "../../src/tool.js";

describe("mcp()", () => {
  const submit = tool({
    name: "submit",
    description: "submit",
    inputSchema: {},
    handler: async () => ({ content: [] }),
  });

  it("returns a handle whose source is an inline http McpSourceSpec", async () => {
    await using handle = await mcp({ name: "weather", tools: [submit] });
    expect(handle.spec).toEqual({
      type: "inline",
      servers: {
        weather: {
          type: "http",
          url: expect.stringMatching(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/),
          headers: { Authorization: expect.stringMatching(/^Bearer .+$/) },
        },
      },
    });
  });

  it("serves the inline source URL while the handle is alive", async () => {
    const handle = await mcp({ name: "alive", tools: [submit] });
    const server = inlineServer(handle.spec, "alive");
    const transport = new StreamableHTTPClientTransport(new URL(server.url!), {
      requestInit: {
        headers: { Authorization: server.headers!.Authorization! },
      },
    });

    const client = new Client({ name: "test", version: "1.0" });
    await client.connect(transport);

    try {
      const tools = await client.listTools();
      expect(tools.tools.map((t) => t.name)).toEqual(["submit"]);
    } finally {
      await client.close();
    }

    await handle[Symbol.asyncDispose]();
    await expect(fetch(server.url!)).rejects.toThrow();
  });

  it("invokes the closure-backed handler and surfaces annotations", async () => {
    let count = 0;
    const increment = tool({
      name: "increment",
      description: "increment a closure-backed counter",
      inputSchema: { delta: z.number() },
      handler: async ({ delta }) => {
        count += delta;
        return { content: [{ type: "text", text: `count=${count}` }] };
      },
      annotations: { title: "Increment", readOnlyHint: false },
    });

    await using handle = await mcp({ name: "counter", tools: [increment] });
    const server = inlineServer(handle.spec, "counter");
    const transport = new StreamableHTTPClientTransport(new URL(server.url!), {
      requestInit: {
        headers: { Authorization: server.headers!.Authorization! },
      },
    });

    const client = new Client({ name: "test", version: "1.0" });
    await client.connect(transport);

    try {
      const tools = await client.listTools();
      expect(tools.tools).toHaveLength(1);
      expect(tools.tools[0]?.annotations?.title).toBe("Increment");

      await client.callTool({ name: "increment", arguments: { delta: 3 } });
      await client.callTool({ name: "increment", arguments: { delta: 4 } });
    } finally {
      await client.close();
    }

    expect(count).toBe(7);
  });

  it("rejects requests without the bearer token (401)", async () => {
    await using handle = await mcp({ name: "guarded", tools: [submit] });
    const { url } = inlineServer(handle.spec, "guarded");

    const response = await fetch(url!, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
    });
    expect(response.status).toBe(401);
  });

  it("rejects requests with the wrong bearer token (401)", async () => {
    await using handle = await mcp({ name: "guarded", tools: [submit] });
    const { url } = inlineServer(handle.spec, "guarded");

    const response = await fetch(url!, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
        Authorization: "Bearer not-the-real-token",
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
    });
    expect(response.status).toBe(401);
  });

  it("rejects requests whose Host header is not allowed (DNS rebinding protection)", async () => {
    await using handle = await mcp({ name: "guarded", tools: [submit] });
    const { url, headers } = inlineServer(handle.spec, "guarded");

    const response = await fetch(url!, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
        Authorization: headers!.Authorization!,
        Host: "evil.example.com",
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
    });
    expect(response.ok).toBe(false);
    expect(response.status).toBeGreaterThanOrEqual(400);
  });

  it("dispose force-closes active HTTP connections", async () => {
    await using handle = await mcp({ name: "closer", tools: [submit] });
    const { url, headers } = inlineServer(handle.spec, "closer");
    const token = headers!.Authorization!.slice("Bearer ".length);
    const socket = await openUnfinishedMcpRequest(url!, token);
    const socketClosed = new Promise<void>((resolve) => {
      socket.once("close", () => resolve());
    });

    try {
      await handle[Symbol.asyncDispose]();
      await socketClosed;
      expect(socket.destroyed).toBe(true);
    } finally {
      socket.destroy();
    }
  });

  it("rejects a name containing the server delimiter", async () => {
    await expect(mcp({ name: "bad__prefix", tools: [] })).rejects.toThrow(
      /must not contain/,
    );
  });

  it("rejects duplicate tool names within a group", async () => {
    const first = tool({
      name: "submit",
      description: "submit",
      inputSchema: {},
      handler: async () => ({ content: [] }),
    });

    const second = tool({
      name: "submit",
      description: "submit again",
      inputSchema: {},
      handler: async () => ({ content: [] }),
    });

    await expect(
      mcp({ name: "custom", tools: [first, second] }),
    ).rejects.toThrow(/duplicate tool name/);
  });

  it("produces independent servers on independent ports", async () => {
    await using first = await mcp({ name: "first", tools: [submit] });
    await using second = await mcp({ name: "second", tools: [submit] });

    const firstUrl = inlineServer(first.spec, "first").url;
    const secondUrl = inlineServer(second.spec, "second").url;
    expect(firstUrl).not.toBe(secondUrl);
  });

  it("supports an empty tools array (a server with no tools)", async () => {
    await using handle = await mcp({ name: "empty", tools: [] });
    expect(inlineServer(handle.spec, "empty")).toBeDefined();
  });

  it("dispose is idempotent", async () => {
    const handle = await mcp({ name: "once", tools: [submit] });
    await handle[Symbol.asyncDispose]();
    await expect(handle[Symbol.asyncDispose]()).resolves.toBeUndefined();
  });

  function inlineServer(
    source: InlineMcpSource,
    name: string,
  ): { type?: string; url?: string; headers?: Record<string, string> } {
    const server = source.servers[name];
    if (!server) throw new Error(`missing server ${name}`);
    return server;
  }
});

async function openUnfinishedMcpRequest(
  urlString: string,
  authToken: string,
): Promise<Socket> {
  const url = new URL(urlString);
  const socket = connect(Number(url.port), url.hostname);
  socket.on("error", () => undefined);
  await once(socket, "connect");
  socket.write(
    [
      `POST ${url.pathname} HTTP/1.1`,
      `Host: ${url.host}`,
      `Authorization: Bearer ${authToken}`,
      "Content-Type: application/json",
      "Accept: application/json",
      "Content-Length: 1000000",
      "",
      "{",
    ].join("\r\n"),
  );
  return socket;
}
