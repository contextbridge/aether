import { cwd as processCwd } from "node:process";

import {
  buildAetherCliCommand,
  type AetherCliCommand,
} from "./agentProcess.js";
import { runCommand } from "./childProcess.js";
import { assertOptionInvariants, compactCliOptions } from "./cliOptions.js";
import { throwIfAborted } from "./errors.js";
import type { AetherHeadlessCliOptions } from "./generated/aether-headless-options.js";
import { resolveEnv } from "./processEnv.js";

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
  stdout?: HeadlessStdioMode;
  stderr?: HeadlessStdioMode;
  abortSignal?: AbortSignal;
}

export async function runHeadless(
  options: AetherHeadlessOptions,
): Promise<AetherHeadlessResult> {
  throwIfAborted(options.abortSignal);
  const { command, args } = buildHeadlessCommand(options);
  return runHeadlessProcess(command, args, options);
}

function buildHeadlessCommand(
  options: AetherHeadlessOptions,
): AetherCliCommand {
  const { binaryPath, stdout, stderr, abortSignal, env, ...cliOptions } =
    options;

  assertOptionInvariants(cliOptions);

  return buildAetherCliCommand({
    binaryPath,
    subcommand: "headless",
    options: compactCliOptions(cliOptions),
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
