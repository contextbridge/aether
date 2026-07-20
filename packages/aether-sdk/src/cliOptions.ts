import { AetherSdkError } from "./errors.js";

export function assertOptionInvariants(options: {
  settings?: unknown;
  settingsFile?: unknown;
  agent?: unknown;
  model?: unknown;
  reasoningEffort?: unknown;
}): void {
  if (options.settings && options.settingsFile) {
    throw new AetherSdkError(
      "invalid_options",
      "settings and settingsFile cannot both be supplied",
    );
  }
  if (options.agent && options.model) {
    throw new AetherSdkError(
      "invalid_options",
      "agent and model cannot both be supplied",
    );
  }
  if (options.reasoningEffort && !options.model) {
    throw new AetherSdkError(
      "invalid_options",
      "reasoningEffort requires model",
    );
  }
}

/**
 * Drop `undefined` entries from a CLI options object and sort the `providers` record so the
 * serialized `--options-json` is stable. Shared by the `aether acp` and `aether headless` command
 * builders so both produce identical option encodings.
 */
export function compactCliOptions(options: object): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(options)
      .filter(([, value]) => value !== undefined)
      .map(([key, value]) => [
        key,
        key === "providers" ? sortRecord(value) : value,
      ]),
  );
}

function sortRecord<T>(value: T): T {
  if (!value || typeof value !== "object" || Array.isArray(value)) return value;
  return Object.fromEntries(Object.entries(value).sort()) as T;
}
