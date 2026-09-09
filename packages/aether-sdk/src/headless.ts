import { cwd as processCwd } from "node:process";

import { buildAetherCliCommand } from "./agentProcess.js";
import { splitLines, streamCommand } from "./childProcess.js";
import { assertOptionInvariants, compactCliOptions } from "./cliOptions.js";
import { AetherSdkError } from "./errors.js";
import type { AetherHeadlessCliOptions } from "./generated/aether-headless-options.js";
import type { AgentEvent } from "./generated/eval-types.js";
import { resolveEnv } from "./processEnv.js";

export type HeadlessEventKind = NonNullable<
  AetherHeadlessCliOptions["events"]
>[number];

export interface AetherHeadlessOptions extends Omit<
  AetherHeadlessCliOptions,
  "mcpConfig" | "prompt" | "output"
> {
  prompt: string;
  binaryPath?: string;
  env?: Record<string, string | undefined>;
  abortSignal?: AbortSignal;
}

/** Stream headless AgentEvents, terminating the child when iteration ends early. */
export async function* runHeadless(
  options: AetherHeadlessOptions,
): AsyncGenerator<AgentEvent, void, unknown> {
  const { binaryPath, abortSignal, env, ...cliOptions } = options;
  assertOptionInvariants(cliOptions);
  const { command, args } = buildAetherCliCommand({
    binaryPath,
    subcommand: "headless",
    options: compactCliOptions({ ...cliOptions, output: "json" }),
  });
  const lines = streamCommand(
    command,
    args,
    {
      cwd: options.cwd ?? processCwd(),
      env: resolveEnv(env),
      abortSignal,
      spawnFailedMessage: `Failed to spawn aether headless at ${command}`,
      exitedErrorCode: "process_exited",
      exitedMessage: ({ exitCode, signal, stderr }) =>
        `aether headless exited with code=${exitCode} signal=${signal}\n${stderr}`,
    },
    splitLines,
  );
  for await (const line of lines) {
    if (line.trim()) yield parseEvent(line);
  }
}

function parseEvent(line: string): AgentEvent {
  try {
    const value: unknown = JSON.parse(line);
    if (
      !isRecord(value) ||
      typeof value.category !== "string" ||
      !isRecord(value.event)
    ) {
      throw new Error("Expected an AgentEvent envelope");
    }
    return value as AgentEvent;
  } catch (cause) {
    throw new AetherSdkError(
      "invalid_protocol_message",
      `Invalid headless AgentEvent: ${line}`,
      cause,
    );
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
