import type * as acp from "@agentclientprotocol/sdk";

import { AetherSdkError } from "../errors.js";
import type { McpServerConfig } from "../generated/aether-headless-options.js";
import { LocalMcpServerHost } from "./localMcpServer.js";
import type {
  AetherToolGroups,
  ExternalMcpServerConfig,
  SdkMcpToolDefinition,
} from "../types.js";

export interface McpSessionConfig {
  externalMcpServers?: Record<string, ExternalMcpServerConfig>;
  tools?: AetherToolGroups;
}

export interface StartedMcpServers {
  acpServers: acp.McpServer[];
  cleanup: () => Promise<void>;
}

export interface StartedHeadlessMcpServers {
  mcpConfig: { servers: Record<string, McpServerConfig> };
  cleanup: () => Promise<void>;
}

export async function startMcpServersForSession(
  input: McpSessionConfig = {},
): Promise<StartedMcpServers> {
  const started = await startHosts(input);
  return { acpServers: toAcpServers(started), cleanup: started.cleanup };
}

export async function startMcpServersForHeadless(
  input: McpSessionConfig = {},
): Promise<StartedHeadlessMcpServers> {
  const started = await startHosts(input);
  return { mcpConfig: toHeadlessMcpConfig(started), cleanup: started.cleanup };
}

interface StartedToolHost {
  name: string;
  url: string;
  authToken: string;
}

interface StartedHosts {
  toolServers: StartedToolHost[];
  externalServers: { name: string; config: ExternalMcpServerConfig }[];
  cleanup: () => Promise<void>;
}

async function startHosts(input: McpSessionConfig): Promise<StartedHosts> {
  const { externalMcpServers, tools } = input;
  const hosts: LocalMcpServerHost[] = [];
  const cleanup = async () => {
    await Promise.allSettled(hosts.map((h) => h.stop()));
  };

  if (isEmpty(externalMcpServers) && isEmpty(tools)) {
    return { toolServers: [], externalServers: [], cleanup };
  }

  try {
    validateServerNames(externalMcpServers, tools);

    const toolServers = await Promise.all(
      Object.entries(tools ?? {}).map(([name, definitions]) =>
        startSdkToolGroup(name, definitions, hosts),
      ),
    );
    const externalServers = Object.entries(externalMcpServers ?? {}).map(
      ([name, config]) => ({ name, config }),
    );
    return { toolServers, externalServers, cleanup };
  } catch (err) {
    await cleanup();
    throw err;
  }
}

async function startSdkToolGroup(
  name: string,
  tools: SdkMcpToolDefinition<any>[],
  hosts: LocalMcpServerHost[],
): Promise<StartedToolHost> {
  validateToolDefinitions(name, tools);
  const host = new LocalMcpServerHost({ name, tools });
  hosts.push(host);
  const { url, authToken } = await host.start();
  return { name, url, authToken };
}

function toAcpServers(started: StartedHosts): acp.McpServer[] {
  return [
    ...started.toolServers.map(toAcpHttpServer),
    ...started.externalServers.map(({ name, config }) =>
      toAcpExternalServer(name, config),
    ),
  ];
}

function toHeadlessMcpConfig(started: StartedHosts): {
  servers: Record<string, McpServerConfig>;
} {
  const servers: Record<string, McpServerConfig> = {};
  for (const { name, authToken, url } of started.toolServers) {
    servers[name] = {
      type: "http",
      url,
      headers: { Authorization: `Bearer ${authToken}` },
    };
  }

  for (const { name, config } of started.externalServers) {
    servers[name] = toServerConfig(config);
  }

  return { servers };
}

function toAcpHttpServer({
  name,
  url,
  authToken,
}: StartedToolHost): acp.McpServer {
  return {
    type: "http",
    name,
    url,
    headers: [{ name: "Authorization", value: `Bearer ${authToken}` }],
  };
}

function toAcpExternalServer(
  name: string,
  config: ExternalMcpServerConfig,
): acp.McpServer {
  switch (config.type) {
    case "http":
    case "sse":
      return { ...toAcpUrlServer(config), name };
    case "stdio":
      return {
        name,
        command: config.command,
        args: config.args ?? [],
        env: toEnvArray(config.env),
      };
  }
}

function toServerConfig(config: ExternalMcpServerConfig): McpServerConfig {
  switch (config.type) {
    case "http":
    case "sse":
      return { ...config, headers: config.headers };
    case "stdio":
      return {
        type: "stdio",
        command: config.command,
        args: config.args ?? [],
        env: filterDefinedEnv(config.env),
      };
  }
}

function toAcpUrlServer(
  config: Extract<ExternalMcpServerConfig, { type: "http" | "sse" }>,
): Omit<Extract<acp.McpServer, { type: "http" | "sse" }>, "name"> {
  return {
    type: config.type,
    url: config.url,
    headers: toHeaderArray(config.headers),
  };
}

function validateServerNames(
  externalMcpServers: Record<string, ExternalMcpServerConfig> | undefined,
  tools: AetherToolGroups | undefined,
): void {
  const seen = new Set<string>();
  for (const name of Object.keys(tools ?? {})) {
    validateServerName(name, `tools.${name}`);
    addUniqueServerName(seen, name);
  }
  for (const name of Object.keys(externalMcpServers ?? {})) {
    validateServerName(name, `externalMcpServers.${name}`);
    addUniqueServerName(seen, name);
  }
}

function validateServerName(name: string, field: string): void {
  if (name.trim().length === 0) {
    throw new AetherSdkError(
      "mcp_server_invalid_config",
      `${field} must be a non-empty MCP server name`,
    );
  }
  if (name.includes("__")) {
    throw new AetherSdkError(
      "mcp_server_invalid_config",
      `${field} must not contain "__"`,
    );
  }
}

function addUniqueServerName(seen: Set<string>, name: string): void {
  if (seen.has(name)) {
    throw new AetherSdkError(
      "mcp_server_invalid_config",
      `Duplicate MCP server name "${name}"`,
    );
  }
  seen.add(name);
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
        `tools.${name} contains a tool with an empty name`,
      );
    }
    if (toolNames.has(definition.name)) {
      throw new AetherSdkError(
        "mcp_server_invalid_config",
        `tools.${name} contains duplicate tool name "${definition.name}"`,
      );
    }
    toolNames.add(definition.name);
  }
}

function isEmpty(value: Record<string, unknown> | undefined): boolean {
  return !value || Object.keys(value).length === 0;
}

function toHeaderArray(
  headers: Record<string, string> | undefined,
): { name: string; value: string }[] {
  return Object.entries(headers ?? {}).map(([name, value]) => ({
    name,
    value,
  }));
}

function toEnvArray(
  env: Record<string, string | undefined> | undefined,
): { name: string; value: string }[] {
  return Object.entries(env ?? {}).flatMap(([name, value]) =>
    value === undefined ? [] : [{ name, value }],
  );
}

function filterDefinedEnv(
  env: Record<string, string | undefined> | undefined,
): Record<string, string> | undefined {
  if (!env) return undefined;
  const entries = Object.entries(env).filter(
    (entry): entry is [string, string] => entry[1] !== undefined,
  );
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}
