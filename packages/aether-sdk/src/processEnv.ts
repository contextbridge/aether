/**
 * Resolve the environment for a spawned child process.
 *
 * When `provided` is `undefined`, the current process environment is inherited. Otherwise the
 * provided map *replaces* the environment wholesale (entries with `undefined` values are dropped);
 * callers that need `PATH` and friends must include them.
 */
export function resolveEnv(
  provided: Record<string, string | undefined> | undefined,
): NodeJS.ProcessEnv {
  if (provided === undefined) return process.env;

  const env: NodeJS.ProcessEnv = {};
  for (const [key, value] of Object.entries(provided)) {
    if (value !== undefined) env[key] = value;
  }
  return env;
}
