import { afterEach, describe, expect, it } from "vitest";
import { z } from "zod";

import {
  startMcpServersForHeadless,
  startMcpServersForSession,
} from "../../src/mcp/config.js";
import { tool } from "../../src/tool.js";
import type { ExternalMcpServerConfig } from "../../src/types.js";

describe("startMcpServersForSession()", () => {
  let cleanup: (() => Promise<void>) | null = null;

  afterEach(async () => {
    await cleanup?.().catch(() => undefined);
    cleanup = null;
  });

  it("returns an empty list when no configs are supplied", async () => {
    const result = await startMcpServersForSession();
    cleanup = result.cleanup;
    expect(result.acpServers).toEqual([]);
  });

  it("converts stdio configs without a type field", async () => {
    const configs: Record<string, ExternalMcpServerConfig> = {
      git: {
        type: "stdio",
        command: "/usr/bin/git-mcp",
        args: ["serve"],
        env: { GIT_TOKEN: "abc", IGNORED: undefined },
      },
    };
    const result = await startMcpServersForSession({
      externalMcpServers: configs,
    });
    cleanup = result.cleanup;
    expect(result.acpServers).toHaveLength(1);
    const server = result.acpServers[0]!;
    expect(server.name).toBe("git");
    expect("type" in server ? server.type : undefined).toBeUndefined();
    expect("command" in server ? server.command : undefined).toBe(
      "/usr/bin/git-mcp",
    );
    if ("env" in server) {
      expect(server.env).toEqual([{ name: "GIT_TOKEN", value: "abc" }]);
    }
  });

  it("converts http configs and includes header arrays", async () => {
    const configs: Record<string, ExternalMcpServerConfig> = {
      remote: {
        type: "http",
        url: "https://example.test/mcp",
        headers: { Authorization: "Bearer x" },
      },
    };
    const result = await startMcpServersForSession({
      externalMcpServers: configs,
    });
    cleanup = result.cleanup;
    const server = result.acpServers[0]!;
    expect(server.name).toBe("remote");
    expect("type" in server ? server.type : undefined).toBe("http");
    expect("headers" in server ? server.headers : []).toEqual([
      { name: "Authorization", value: "Bearer x" },
    ]);
  });

  it("converts sse configs", async () => {
    const configs: Record<string, ExternalMcpServerConfig> = {
      streaming: { type: "sse", url: "https://example.test/sse" },
    };
    const result = await startMcpServersForSession({
      externalMcpServers: configs,
    });
    cleanup = result.cleanup;
    const server = result.acpServers[0]!;
    expect(server.name).toBe("streaming");
    expect("type" in server ? server.type : undefined).toBe("sse");
  });

  it("starts keyed SDK tool groups and produces http ACP entries with bearer auth", async () => {
    const submit = tool({
      name: "submit",
      description: "submit",
      inputSchema: { value: z.string() },
      handler: async () => ({ content: [{ type: "text", text: "ok" }] }),
    });
    const result = await startMcpServersForSession({
      tools: { custom: [submit] },
    });
    cleanup = result.cleanup;
    const server = result.acpServers[0]!;
    expect("type" in server ? server.type : undefined).toBe("http");
    expect(server.name).toBe("custom");
    if ("url" in server) {
      expect(server.url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
    }
    if ("headers" in server) {
      expect(server.headers[0]?.name).toBe("Authorization");
      expect(server.headers[0]?.value).toMatch(/^Bearer .+$/);
    }
  });

  it("starts multiple SDK tool groups", async () => {
    const submit = tool({
      name: "submit",
      description: "submit",
      inputSchema: {},
      handler: async () => ({ content: [] }),
    });
    const approve = tool({
      name: "approve",
      description: "approve",
      inputSchema: {},
      handler: async () => ({ content: [] }),
    });
    const result = await startMcpServersForSession({
      tools: { recommendations: [submit], review: [approve] },
    });
    cleanup = result.cleanup;
    expect(result.acpServers.map((server) => server.name)).toEqual([
      "recommendations",
      "review",
    ]);
  });

  it("rejects SDK tool prefixes containing the server delimiter", async () => {
    await expect(
      startMcpServersForSession({ tools: { bad__prefix: [] } }),
    ).rejects.toThrow(/must not contain/);
  });

  it("rejects duplicate tool names within a prefix", async () => {
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
      startMcpServersForSession({ tools: { custom: [first, second] } }),
    ).rejects.toThrow(/duplicate tool name/);
  });

  it("rejects when SDK tool prefixes collide with external MCP server names", async () => {
    const configs: Record<string, ExternalMcpServerConfig> = {
      custom: { type: "stdio", command: "external-mcp" },
    };
    await expect(
      startMcpServersForSession({
        tools: { custom: [] },
        externalMcpServers: configs,
      }),
    ).rejects.toThrow(/Duplicate MCP server name/);
  });
});

describe("startMcpServersForHeadless()", () => {
  let cleanup: (() => Promise<void>) | null = null;

  afterEach(async () => {
    await cleanup?.().catch(() => undefined);
    cleanup = null;
  });

  it("returns an empty server map when no configs are supplied", async () => {
    const result = await startMcpServersForHeadless();
    cleanup = result.cleanup;
    expect(result.mcpConfig).toEqual({ servers: {} });
    expect(result).not.toHaveProperty("acpServers");
  });

  it("emits an http server with bearer auth for an SDK tool group", async () => {
    const submit = tool({
      name: "submit",
      description: "submit",
      inputSchema: { value: z.string() },
      handler: async () => ({ content: [{ type: "text", text: "ok" }] }),
    });
    const result = await startMcpServersForHeadless({
      tools: { weather: [submit] },
    });
    cleanup = result.cleanup;

    const server = result.mcpConfig.servers.weather!;
    expect(result).not.toHaveProperty("acpServers");
    expect(server).toMatchObject({ type: "http" });
    if (server.type === "http") {
      expect(server.url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
      expect(server.headers?.Authorization).toMatch(/^Bearer .+$/);
    }
  });

  it("maps external http and stdio servers into the headless shape", async () => {
    const configs: Record<string, ExternalMcpServerConfig> = {
      remote: {
        type: "http",
        url: "https://example.test/mcp",
        headers: { Authorization: "Bearer x" },
      },
      git: {
        type: "stdio",
        command: "/usr/bin/git-mcp",
        args: ["serve"],
        env: { GIT_TOKEN: "abc", IGNORED: undefined },
      },
    };
    const result = await startMcpServersForHeadless({
      externalMcpServers: configs,
    });
    cleanup = result.cleanup;

    expect(result.mcpConfig.servers.remote).toEqual({
      type: "http",
      url: "https://example.test/mcp",
      headers: { Authorization: "Bearer x" },
    });
    expect(result.mcpConfig.servers.git).toEqual({
      type: "stdio",
      command: "/usr/bin/git-mcp",
      args: ["serve"],
      env: { GIT_TOKEN: "abc" },
    });
  });

  it("stops the local http server on cleanup", async () => {
    const submit = tool({
      name: "submit",
      description: "submit",
      inputSchema: {},
      handler: async () => ({ content: [] }),
    });
    const result = await startMcpServersForHeadless({
      tools: { weather: [submit] },
    });
    const server = result.mcpConfig.servers.weather!;
    const url = server.type === "http" ? server.url : "";

    await result.cleanup();

    await expect(fetch(url)).rejects.toThrow();
  });

  it("rejects SDK tool prefixes containing the server delimiter", async () => {
    await expect(
      startMcpServersForHeadless({ tools: { bad__prefix: [] } }),
    ).rejects.toThrow(/must not contain/);
  });
});
