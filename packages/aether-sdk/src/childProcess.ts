import { spawn, type StdioOptions } from "node:child_process";
import { addAbortListener } from "node:events";

import {
  AetherSdkError,
  type AetherSdkErrorCode,
  throwIfAborted,
} from "./errors.js";
import { stopChild } from "./agentProcess.js";

export type ProcessOutputMode = "pipe" | "inherit";

export interface RunCommandOutput {
  stdout: string;
  stderr: string;
  exitCode: number;
  signal: NodeJS.Signals | null;
}

export interface RunCommandOptions {
  cwd: string;
  env: Record<string, string | undefined>;
  stdin?: string;
  stdout?: ProcessOutputMode;
  stderr?: ProcessOutputMode;
  abortSignal?: AbortSignal;
  spawnFailedMessage: string;
  exitedErrorCode: AetherSdkErrorCode;
  exitedMessage: (result: RunCommandOutput) => string;
}

export function runCommand(
  command: string,
  args: string[],
  options: RunCommandOptions,
): Promise<RunCommandOutput> {
  throwIfAborted(options.abortSignal);

  return new Promise((resolve, reject) => {
    const stdoutMode = options.stdout ?? "pipe";
    const stderrMode = options.stderr ?? "pipe";
    const stdio: StdioOptions = [
      options.stdin === undefined ? "ignore" : "pipe",
      stdoutMode,
      stderrMode,
    ];
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn(command, args, {
        cwd: options.cwd,
        env: options.env,
        stdio,
      });
    } catch (err) {
      reject(
        new AetherSdkError(
          "process_spawn_failed",
          options.spawnFailedMessage,
          err,
        ),
      );
      return;
    }

    let stdout = "";
    let stderr = "";
    if (stdoutMode === "pipe") {
      child.stdout?.setEncoding("utf8");
      child.stdout?.on("data", (chunk: string) => (stdout += chunk));
    }
    if (stderrMode === "pipe") {
      child.stderr?.setEncoding("utf8");
      child.stderr?.on("data", (chunk: string) => (stderr += chunk));
    }
    if (options.stdin !== undefined) {
      child.stdin?.end(options.stdin);
    }

    const abortCleanup = options.abortSignal
      ? addAbortListener(options.abortSignal, () => {
          void stopChild(child);
          reject(new AetherSdkError("aborted", "Aborted by caller"));
        })
      : null;

    child.on("error", (err) => {
      abortCleanup?.[Symbol.dispose]();
      reject(
        new AetherSdkError(
          "process_spawn_failed",
          options.spawnFailedMessage,
          err,
        ),
      );
    });

    child.on("close", (code, signal) => {
      abortCleanup?.[Symbol.dispose]();
      const result = { stdout, stderr, exitCode: code ?? -1, signal };
      if (code === 0) {
        resolve(result);
      } else {
        reject(
          new AetherSdkError(
            options.exitedErrorCode,
            options.exitedMessage(result),
          ),
        );
      }
    });
  });
}
