import { resolveAetherCommand } from "../agentProcess.js";
import { runCommand } from "../childProcess.js";
import { AetherSdkError } from "../errors.js";
import { resolveEnv } from "../processEnv.js";

export interface GenerateOptions {
  /**
   * Model to call, as `provider:model` (e.g. `anthropic:claude-sonnet-4-5`). Defaults to the
   * `AETHER_LLM_MODEL` environment variable.
   */
  model?: string;

  /** Optional system prompt. */
  system?: string;

  /** Path to the `aether` binary. Defaults to the bundled `@aether-agent/cli`. */
  binaryPath?: string;

  /** Environment for the spawned process. Defaults to the current process environment. */
  env?: Record<string, string | undefined>;

  /** Abort the call. */
  signal?: AbortSignal;
}

export interface GenerateResult {
  /** The model's response text. */
  text: string;
}

/**
 * Call a model with a single prompt and return its response, via the `aether generate` CLI. This is
 * the low-level primitive that {@link judge} builds on.
 *
 * @example
 * ```ts
 * const { text } = await generate("Summarize this diff in one sentence:\n" + diff, {
 *   model: "anthropic:claude-sonnet-4-5",
 * });
 * ```
 */
export async function generate(
  prompt: string,
  options: GenerateOptions = {},
): Promise<GenerateResult> {
  const model = options.model ?? process.env.AETHER_LLM_MODEL;
  if (!model) {
    throw new AetherSdkError(
      "invalid_options",
      "No model provided to `generate`. Pass `options.model` or set the AETHER_LLM_MODEL environment variable.",
    );
  }

  const { command, prefixArgs } = resolveAetherCommand(options.binaryPath);
  const args = [
    ...prefixArgs,
    "generate",
    "--model",
    model,
    "--prompt-file",
    "-",
    "--output",
    "json",
  ];
  if (options.system !== undefined) args.push("--system", options.system);

  const { stdout } = await runCommand(command, args, {
    cwd: process.cwd(),
    env: resolveEnv(options.env),
    stdin: prompt,
    abortSignal: options.signal,
    spawnFailedMessage: `Failed to run aether generate at ${command}`,
    exitedErrorCode: "generate_command_failed",
    exitedMessage: ({ exitCode, stderr }) =>
      `aether generate exited with code ${exitCode}.\n${stderr}`,
  });

  return { text: parseResponseText(stdout) };
}

function parseResponseText(stdout: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout);
  } catch (err) {
    throw new AetherSdkError(
      "generate_command_failed",
      `Failed to parse aether generate output as JSON: ${stdout}`,
      err,
    );
  }
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    typeof (parsed as { text?: unknown }).text !== "string"
  ) {
    throw new AetherSdkError(
      "generate_command_failed",
      `aether generate output did not contain a string "text" field: ${stdout}`,
    );
  }
  return (parsed as { text: string }).text;
}
