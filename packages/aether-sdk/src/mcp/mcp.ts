import { AetherSdkError } from "../errors.js";
import type { McpSourceSpecObject } from "../generated/aether-settings.js";
import type { SdkMcpToolDefinition } from "../types.js";
import { LocalMcpServerHost, LocalMcpServerInfo } from "./localMcpServer.js";

export type InlineMcpSource = Extract<McpSourceSpecObject, { type: "inline" }>;

export interface McpHandle extends AsyncDisposable {
  readonly spec: InlineMcpSource;
}

export async function mcp(input: {
  name: string;
  tools: SdkMcpToolDefinition<any>[];
}): Promise<McpHandle> {
  const { name, tools } = input;
  validateServerName(name);
  validateToolDefinitions(name, tools);

  const host = new LocalMcpServerHost({ name, tools });
  let info: LocalMcpServerInfo;
  try {
    info = await host.start();
  } catch (err) {
    await host.stop();
    throw err;
  }
  let disposed = false;

  return {
    spec: {
      type: "inline",
      servers: {
        [name]: {
          type: "http",
          url: info.url,
          headers: { Authorization: `Bearer ${info.authToken}` },
        },
      },
    },
    async [Symbol.asyncDispose]() {
      if (disposed) return;
      disposed = true;
      await host.stop();
    },
  };
}

function validateServerName(name: string): void {
  if (name.trim().length === 0) {
    throw new AetherSdkError(
      "mcp_server_invalid_config",
      "mcp() name must be a non-empty MCP server name",
    );
  }
  if (name.includes("__")) {
    throw new AetherSdkError(
      "mcp_server_invalid_config",
      `mcp() name "${name}" must not contain "__"`,
    );
  }
}

function validateToolDefinitions(
  name: string,
  tools: SdkMcpToolDefinition<any>[],
): void {
  const toolNames = new Set<string>();
  for (const definition of tools) {
    if (definition.name.trim().length === 0) {
      throw new AetherSdkError(
        "mcp_server_invalid_config",
        `mcp("${name}") contains a tool with an empty name`,
      );
    }
    if (toolNames.has(definition.name)) {
      throw new AetherSdkError(
        "mcp_server_invalid_config",
        `mcp("${name}") contains duplicate tool name "${definition.name}"`,
      );
    }
    toolNames.add(definition.name);
  }
}
