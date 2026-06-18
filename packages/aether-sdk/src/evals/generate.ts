import { z } from "zod";

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

export type GenerateJsonOptions<T extends z.ZodType> = GenerateOptions & {
  /** Validate the model's response as JSON with this schema and return the parsed value. */
  schema: T;
};

export interface GenerateResult {
  /** The model's response text. */
  text: string;
}

/**
 * Call a model with a single prompt and return its response, via the `aether generate` CLI.
 * When a Zod schema is provided, the model response is parsed as JSON, validated, and returned
 * as the inferred schema type.
 *
 * @example
 * ```ts
 * const { text } = await generate("Summarize this diff in one sentence:\n" + diff, {
 *   model: "anthropic:claude-sonnet-4-5",
 * });
 *
 * const verdict = await generate("Grade this run as JSON", {
 *   model: "anthropic:claude-sonnet-4-5",
 *   schema: z.object({ passed: z.boolean(), reason: z.string() }),
 * });
 * ```
 */
export function generate(
  prompt: string,
  options?: GenerateOptions,
): Promise<GenerateResult>;
export function generate<T extends z.ZodType>(
  prompt: string,
  options: GenerateJsonOptions<T>,
): Promise<z.infer<T>>;
export async function generate<T extends z.ZodType>(
  prompt: string,
  options: GenerateOptions | GenerateJsonOptions<T> = {},
): Promise<GenerateResult | z.infer<T>> {
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

  const promptWithInstructions =
    "schema" in options
      ? `${prompt}\n\nRespond with ONLY valid JSON. Do not include markdown fences or explanatory prose.`
      : prompt;

  const { stdout } = await runCommand(command, args, {
    cwd: process.cwd(),
    env: resolveEnv(options.env),
    stdin: promptWithInstructions,
    abortSignal: options.signal,
    spawnFailedMessage: `Failed to run aether generate at ${command}`,
    exitedErrorCode: "generate_command_failed",
    exitedMessage: ({ exitCode, stderr }) =>
      `aether generate exited with code ${exitCode}.\n${stderr}`,
  });

  const text = parseResponseText(stdout);
  if ("schema" in options) return parseJsonResponse(text, options.schema);
  return { text };
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

function parseJsonResponse<T extends z.ZodType>(
  response: string,
  schema: T,
): z.infer<T> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(extractJson(response));
  } catch (err) {
    throw new AetherSdkError(
      "generate_command_failed",
      `Model returned invalid JSON.\nRaw response: ${response}`,
      err,
    );
  }

  const result = schema.safeParse(parsed);
  if (!result.success) {
    throw new AetherSdkError(
      "generate_command_failed",
      `Model response did not match schema.\n${z.prettifyError(result.error)}\nRaw response: ${response}`,
      result.error,
    );
  }
  return result.data;
}

function extractJson(response: string): string {
  const trimmed = response.trim();
  try {
    JSON.parse(trimmed);
    return trimmed;
  } catch {
    const objectStart = trimmed.indexOf("{");
    const objectEnd = trimmed.lastIndexOf("}");
    if (objectStart !== -1 && objectEnd !== -1 && objectStart <= objectEnd) {
      return trimmed.slice(objectStart, objectEnd + 1);
    }

    const arrayStart = trimmed.indexOf("[");
    const arrayEnd = trimmed.lastIndexOf("]");
    if (arrayStart !== -1 && arrayEnd !== -1 && arrayStart <= arrayEnd) {
      return trimmed.slice(arrayStart, arrayEnd + 1);
    }

    return trimmed;
  }
}
