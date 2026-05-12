import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import type { AsyncQueue } from "./asyncQueue.js";
import { AetherSdkError } from "./errors.js";
import type { AetherMessage } from "./types.js";

const TERMINATION_GRACE_MS = 1_000;

export interface SpawnAetherProcessOptions {
  command: string;
  args: string[];
  cwd: string;
  env: Record<string, string | undefined> | undefined;
  events: AsyncQueue<AetherMessage>;
}

export interface SpawnedAetherProcess {
  child: ChildProcess;
  stdin: NonNullable<ChildProcess["stdin"]>;
  stdout: NonNullable<ChildProcess["stdout"]>;
}

export interface ResolvedAetherCommand {
  command: string;
  prefixArgs: string[];
}

/**
 * Pick the aether executable to spawn.
 *
 * When `binaryPath` is given, it is used verbatim. Otherwise the bundled
 * `@aether-agent/cli` shim is resolved through Node's module resolution and
 * launched with the current Node binary so the lookup works cross-platform
 * without relying on a shebang.
 */
export function resolveAetherCommand(
  binaryPath: string | undefined,
): ResolvedAetherCommand {
  if (binaryPath) return { command: binaryPath, prefixArgs: [] };
  const require = createRequire(import.meta.url);
  const pkgJsonPath = require.resolve("@aether-agent/cli/package.json");
  const pkg = require("@aether-agent/cli/package.json") as {
    bin: { aether: string };
  };
  return {
    command: process.execPath,
    prefixArgs: [join(dirname(pkgJsonPath), pkg.bin.aether)],
  };
}

export function spawnAetherProcess({
  command,
  args,
  cwd,
  env,
  events,
}: SpawnAetherProcessOptions): SpawnedAetherProcess {
  let child: ChildProcess;
  try {
    child = spawn(command, args, {
      cwd,
      env: resolveEnv(env),
      stdio: ["pipe", "pipe", "inherit"],
    });
  } catch (err) {
    throw new AetherSdkError(
      "process_spawn_failed",
      `Failed to spawn aether process at ${command}`,
      err,
    );
  }

  const { stdin, stdout } = child;
  if (!stdin || !stdout) {
    void stopChild(child);
    throw new AetherSdkError(
      "process_spawn_failed",
      "aether process is missing stdio pipes",
    );
  }

  child.on("error", (err) => {
    events.fail(
      new AetherSdkError("process_exited", "aether subprocess error", err),
    );
  });

  child.on("exit", (code, signal) => {
    if (code !== 0 && signal !== "SIGTERM" && signal !== "SIGINT") {
      events.fail(
        new AetherSdkError(
          "process_exited",
          `aether subprocess exited with code=${code} signal=${signal}`,
        ),
      );
    } else {
      events.close();
    }
  });

  return { child, stdin, stdout };
}

export async function stopChild(child: ChildProcess): Promise<void> {
  if (hasExited(child)) return;

  const exited = once(child, "exit")
    .then(() => undefined)
    .catch(() => undefined);
  try {
    child.kill("SIGTERM");
  } catch {}

  await Promise.race([
    exited,
    sleep(TERMINATION_GRACE_MS, undefined, { ref: false }).then(() => {
      if (!hasExited(child)) {
        try {
          child.kill("SIGKILL");
        } catch {}
      }
    }),
  ]);

  if (!hasExited(child)) await exited;
}

function hasExited(child: ChildProcess): boolean {
  return child.exitCode !== null || child.signalCode !== null;
}

function resolveEnv(
  provided: Record<string, string | undefined> | undefined,
): NodeJS.ProcessEnv {
  if (provided === undefined) return process.env;

  const env: NodeJS.ProcessEnv = {};
  for (const [key, value] of Object.entries(provided)) {
    if (value !== undefined) env[key] = value;
  }
  return env;
}
