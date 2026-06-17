import { cwd as processCwd } from "node:process";

import {
  buildAetherCliCommand,
  type AetherCliCommand,
} from "./agentProcess.js";
import { runCommand } from "./childProcess.js";
import { assertOptionInvariants, compactCliOptions } from "./cliOptions.js";
import { throwIfAborted } from "./errors.js";
import type {
  AetherHeadlessCliOptions,
  McpConfig,
} from "./generated/aether-headless-options.js";
import { startMcpServersForHeadless } from "./mcp/index.js";
import { resolveEnv } from "./processEnv.js";
import type { AetherToolGroups, ExternalMcpServerConfig } from "./types.js";

export type HeadlessOutputFormat = NonNullable<
  AetherHeadlessCliOptions["output"]
>;

export type HeadlessEventKind = NonNullable<
  AetherHeadlessCliOptions["events"]
>[number];

export type HeadlessStdioMode = "pipe" | "inherit";

export interface AetherHeadlessResult {
  stdout: string;
  stderr: string;
  exitCode: number;
  signal: NodeJS.Signals | null;
}

export interface AetherHeadlessOptions extends Omit<
  AetherHeadlessCliOptions,
  "mcpConfig" | "prompt"
> {
  prompt: string;
  binaryPath?: string;
  env?: Record<string, string | undefined>;
  tools?: AetherToolGroups;
  externalMcpServers?: Record<string, ExternalMcpServerConfig>;
  stdout?: HeadlessStdioMode;
  stderr?: HeadlessStdioMode;
  abortSignal?: AbortSignal;
}

export async function runHeadless(
  options: AetherHeadlessOptions,
): Promise<AetherHeadlessResult> {
  throwIfAborted(options.abortSignal);
  const started = await startMcpServersForHeadless({
    tools: options.tools,
    externalMcpServers: options.externalMcpServers,
  });
  try {
    throwIfAborted(options.abortSignal);
    const { command, args } = buildHeadlessCommand(options, started.mcpConfig);
    return await runHeadlessProcess(command, args, options);
  } finally {
    await started.cleanup();
  }
}

function buildHeadlessCommand(
  options: AetherHeadlessOptions,
  mcpConfig: McpConfig = { servers: {} },
): AetherCliCommand {
  const {
    binaryPath,
    tools,
    externalMcpServers,
    stdout,
    stderr,
    abortSignal,
    env,
    ...cliOptions
  } = options;

  assertOptionInvariants(cliOptions);

  return buildAetherCliCommand({
    binaryPath,
    subcommand: "headless",
    options: compactCliOptions({
      ...cliOptions,
      mcpConfig:
        Object.keys(mcpConfig.servers).length > 0 ? mcpConfig : undefined,
    }),
  });
}

function runHeadlessProcess(
  command: string,
  args: string[],
  options: AetherHeadlessOptions,
): Promise<AetherHeadlessResult> {
  return runCommand(command, args, {
    cwd: options.cwd ?? processCwd(),
    env: resolveEnv(options.env),
    stdout: options.stdout,
    stderr: options.stderr,
    abortSignal: options.abortSignal,
    spawnFailedMessage: `Failed to spawn aether headless at ${command}`,
    exitedErrorCode: "process_exited",
    exitedMessage: ({ exitCode, signal, stderr }) =>
      `aether headless exited with code=${exitCode} signal=${signal}\n${stderr}`,
  });
}
