import { cwd as processCwd } from "node:process";
import { resolveAetherCommand } from "../agentProcess.js";
import { runCommand } from "../childProcess.js";
import { AetherSdkError } from "../errors.js";
import { resolveEnv } from "../processEnv.js";
import type {
  AgentMessage,
  EvalOutcome,
  EvalSpec,
  EvalStreamEvent,
  JudgeCriterionSummary,
  JudgeSummary,
} from "../generated/eval-types.js";
import { createWorkspaceHandle, type WorkspaceHandle } from "./workspace.js";

export type {
  AgentMessage,
  EvalSpec,
  EvalOutcome,
  JudgeSummary,
  JudgeCriterionSummary,
};
export type EvalRunSpec = Omit<EvalSpec, "expect">;

export interface RunEvalOptions {
  /** Path to the `aether` binary. Defaults to the bundled `@aether-agent/cli`. */
  binaryPath?: string;

  /**
   * Base directory for resolving relative paths in the spec (Docker build context, `dir`
   * workspace, settings/judge file paths). Defaults to the current working directory.
   */
  baseDir?: string;

  /** Environment for the spawned process. Defaults to the current process environment. */
  env?: Record<string, string | undefined>;

  /** Retain the workspace when the result is disposed instead of deleting it, for debugging. */
  keepWorkspace?: boolean;

  /** Abort the eval run. */
  signal?: AbortSignal;

  /** Called for each `AgentMessage` the eval emits, as soon as it arrives. */
  onMessage?: (message: AgentMessage) => void;

  /** Called with each raw stderr chunk produced by the eval process. */
  onStderr?: (chunk: string) => void;
}

export interface EvalRunResult
  extends Omit<EvalOutcome, "retainedWorkspace">, AsyncDisposable {
  /** Handle to the retained workspace; dispose (or `cleanup()`) removes it. */
  readonly workspace: WorkspaceHandle;
}

/**
 * Run a single Dockerized eval via the `aether` CLI and return its outcome plus a disposable handle
 * to the retained workspace. Agent messages and stderr are streamed to the optional `onMessage`
 * and `onStderr` callbacks while the eval runs.
 *
 * @example
 * ```ts
 * await using result = await runEval({
 *   docker: { image: "aether-sandbox:latest" },
 *   name: "edit-notes",
 *   task: {
 *     prompt: "Change the first line of notes.txt from alpha to beta",
 *     workspace: { files: { "notes.txt": "alpha\nalpha\n" } },
 *   },
 * });
 * expect(result.passed).toBe(true);
 * ```
 */
export async function runEval(
  spec: EvalRunSpec,
  options: RunEvalOptions = {},
): Promise<EvalRunResult> {
  if (spec && typeof spec === "object" && "expect" in spec) {
    throw new AetherSdkError(
      "invalid_options",
      "`expect` is not allowed on runEval specs. Assert against the returned EvalRunResult directly instead (e.g. result.passed, result.judge, result.toolCalls).",
    );
  }

  const baseDir = options.baseDir ?? processCwd();
  const { command, prefixArgs } = resolveAetherCommand(options.binaryPath);
  const childEnv = resolveEnv(options.env);
  const consumer = createEvalEventConsumer(options);
  const { stderr } = await runCommand(
    command,
    [
      ...prefixArgs,
      "eval",
      "--spec-file",
      "-",
      "--retain-workspace",
      "--base-dir",
      baseDir,
    ],
    {
      cwd: baseDir,
      env: childEnv,
      stdin: JSON.stringify(spec),
      abortSignal: options.signal,
      onStdout: (chunk) => consumer.push(chunk),
      onStderr: options.onStderr,
      spawnFailedMessage: `Failed to run aether eval at ${command}`,
      exitedErrorCode: "eval_command_failed",
      exitedMessage: ({ exitCode, stderr }) =>
        `aether eval exited with code ${exitCode}.\n${stderr}`,
    },
  );

  const outcome = consumer.finish(stderr);

  const { retainedWorkspace, ...result } = outcome;

  if (!retainedWorkspace) {
    throw new AetherSdkError(
      "eval_command_failed",
      `aether eval did not report a workspace path.\nstderr:\n${stderr}`,
    );
  }

  const workspace = createWorkspaceHandle(
    retainedWorkspace,
    options.keepWorkspace ?? false,
  );

  return {
    ...result,
    workspace,
    [Symbol.asyncDispose]: () => workspace[Symbol.asyncDispose](),
  };
}

function createEvalEventConsumer(options: RunEvalOptions) {
  let buffered = "";
  let outcome: EvalOutcome | undefined;

  function push(chunk: string): void {
    buffered += chunk;
    const lines = buffered.split(/\r?\n/);
    buffered = lines.pop() ?? "";
    for (const line of lines) processLine(line);
  }

  function finish(stderr: string): EvalOutcome {
    if (buffered.trim()) {
      const pending = buffered;
      buffered = "";
      processLine(pending);
    }
    if (!outcome) {
      throw new AetherSdkError(
        "eval_command_failed",
        `aether eval did not emit an outcome event.\nstderr:\n${stderr}`,
      );
    }
    return outcome;
  }

  function processLine(line: string): void {
    if (!line.trim()) return;
    let parsed: EvalStreamEvent;
    try {
      parsed = JSON.parse(line) as EvalStreamEvent;
    } catch (err) {
      throw new AetherSdkError(
        "eval_command_failed",
        `Failed to parse aether eval event line as JSON: ${line}`,
        err,
      );
    }
    switch (parsed.type) {
      case "agent_message":
        options.onMessage?.(parsed.message);
        return;
      case "outcome":
        if (outcome) {
          throw new AetherSdkError(
            "eval_command_failed",
            "aether eval emitted multiple outcome events",
          );
        }
        outcome = parsed.outcome;
        return;
      default:
        throw new AetherSdkError(
          "eval_command_failed",
          `unknown aether eval event type: ${(parsed as { type?: unknown }).type}`,
        );
    }
  }

  return { push, finish };
}
