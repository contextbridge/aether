import { spawn, type ChildProcessByStdio } from "node:child_process";
import { addAbortListener } from "node:events";
import type { Readable, Writable } from "node:stream";
import { text } from "node:stream/consumers";

import {
  AetherSdkError,
  type AetherSdkErrorCode,
  throwIfAborted,
} from "./errors.js";
import { stopChild } from "./agentProcess.js";

export interface CommandExit {
  exitCode: number | null;
  signal: NodeJS.Signals | null;
  stderr: string;
}

export interface RunCommandOptions {
  cwd: string;
  env: Record<string, string | undefined>;
  stdin?: string;
  abortSignal?: AbortSignal;
  spawnFailedMessage: string;
  exitedErrorCode: AetherSdkErrorCode;
  exitedMessage: (exit: CommandExit) => string;
}

/** Run a command to completion and return its stdout. */
export function runCommand(
  command: string,
  args: string[],
  options: RunCommandOptions,
): Promise<string> {
  return text(streamCommand(command, args, options));
}

/**
 * Spawn a command and stream its stdout.
 */
export async function* streamCommand(
  command: string,
  args: string[],
  options: RunCommandOptions,
  transformStdout: (chunks: AsyncIterable<string>) => AsyncIterable<string> = (
    chunks,
  ) => chunks,
): AsyncGenerator<string, void, unknown> {
  throwIfAborted(options.abortSignal);

  let child: ChildProcessByStdio<Writable | null, Readable, Readable>;
  try {
    const spawnOptions = { cwd: options.cwd, env: options.env };
    child =
      options.stdin === undefined
        ? spawn(command, args, {
            ...spawnOptions,
            stdio: ["ignore", "pipe", "pipe"],
          })
        : spawn(command, args, { ...spawnOptions, stdio: "pipe" });
  } catch (cause) {
    throw spawnFailed(options, cause);
  }

  let failure: AetherSdkError | undefined;
  let stderr = "";
  child.on("error", (cause) => {
    failure = spawnFailed(options, cause);
  });

  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk: string) => {
    stderr += chunk;
  });

  child.stdin?.end(options.stdin);
  const closed = new Promise<Omit<CommandExit, "stderr">>((resolve) => {
    child.once("close", (exitCode, signal) => resolve({ exitCode, signal }));
  });

  const abortCleanup = options.abortSignal
    ? addAbortListener(options.abortSignal, () => void stopChild(child))
    : undefined;

  try {
    child.stdout.setEncoding("utf8");
    for await (const chunk of transformStdout(child.stdout)) {
      throwIfAborted(options.abortSignal);
      yield chunk;
    }
    const { exitCode, signal } = await closed;
    throwIfAborted(options.abortSignal);
    if (failure) throw failure;
    if (exitCode !== 0) {
      throw new AetherSdkError(
        options.exitedErrorCode,
        options.exitedMessage({ exitCode, signal, stderr }),
      );
    }
  } finally {
    abortCleanup?.[Symbol.dispose]();
    await stopChild(child);
    await closed;
  }
}

export async function* splitLines(
  chunks: AsyncIterable<string>,
): AsyncGenerator<string, void, unknown> {
  let pending = "";
  for await (const chunk of chunks) {
    const lines = (pending + chunk).split(/\r?\n/);
    pending = lines.pop()!;
    yield* lines;
  }
  if (pending) yield pending;
}

function spawnFailed(
  options: RunCommandOptions,
  cause: unknown,
): AetherSdkError {
  return new AetherSdkError(
    "process_spawn_failed",
    options.spawnFailedMessage,
    cause,
  );
}
