import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { cwd as processCwd } from "node:process";
import { Readable, Writable } from "node:stream";
import { setTimeout as sleep } from "node:timers/promises";

import * as acp from "@agentclientprotocol/sdk";
import type { AsyncQueue } from "./asyncQueue.js";
import { assertOptionInvariants, compactCliOptions } from "./cliOptions.js";
import { AetherSdkError } from "./errors.js";
import type { AetherAcpOptions } from "./generated/aether-acp-options.js";
import { resolveEnv } from "./processEnv.js";
import type { AetherMessage } from "./types.js";

const TERMINATION_GRACE_MS = 1_000;

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

export type SettingsSelection =
  | {
      settings: NonNullable<AetherAcpOptions["settings"]>;
      settingsFile?: never;
    }
  | { settings?: never; settingsFile: string }
  | { settings?: never; settingsFile?: never };

export interface AetherAcpAgentProcessOptions extends AetherAcpOptions {
  cwd?: string;
  binaryPath?: string;
  env?: Record<string, string | undefined>;
  events?: AsyncQueue<AetherMessage>;
}

export interface AetherAcpCommand {
  command: string;
  args: string[];
}

export interface AetherCliCommand {
  command: string;
  args: string[];
}

export interface AcpAgentProcess extends AsyncDisposable {
  readonly child: ChildProcess;
  readonly stdin: NodeJS.WritableStream;
  readonly stdout: NodeJS.ReadableStream;
  readonly stream: ReturnType<typeof acp.ndJsonStream>;
  close(): Promise<void>;
}

export function buildAetherAcpCommand(
  options: AetherAcpAgentProcessOptions = {},
): AetherAcpCommand {
  const {
    binaryPath,
    settings,
    settingsFile,
    agent,
    model,
    reasoningEffort,
    providers,
    logDir,
  } = options;

  assertOptionInvariants({
    settings,
    settingsFile,
    agent,
    model,
    reasoningEffort,
  });

  return buildAetherCliCommand({
    binaryPath,
    subcommand: "acp",
    options: compactCliOptions({
      logDir,
      providers,
      settings,
      settingsFile,
      agent,
      model,
      reasoningEffort,
    }),
    omitOptionsJsonWhenEmpty: true,
  });
}

export function buildAetherCliCommand(input: {
  binaryPath?: string;
  subcommand: string;
  options: Record<string, unknown>;
  omitOptionsJsonWhenEmpty?: boolean;
}): AetherCliCommand {
  const resolved = resolveAetherCommand(input.binaryPath);
  const args = [...resolved.prefixArgs, input.subcommand];
  if (
    !input.omitOptionsJsonWhenEmpty ||
    Object.keys(input.options).length > 0
  ) {
    args.push("--options-json", JSON.stringify(input.options));
  }
  return { command: resolved.command, args };
}

export function startAgent(
  options: AetherAcpAgentProcessOptions = {},
): AcpAgentProcess {
  const { command, args } = buildAetherAcpCommand(options);
  let child: ChildProcess;
  try {
    child = spawn(command, args, {
      cwd: options.cwd ?? processCwd(),
      env: resolveEnv(options.env),
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
  const { events } = options;
  if (events) {
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
  }

  if (!stdin || !stdout) {
    void stopChild(child);
    throw new AetherSdkError(
      "process_spawn_failed",
      "aether process is missing stdio pipes",
    );
  }

  const stream = acp.ndJsonStream(
    Writable.toWeb(stdin) as WritableStream<Uint8Array>,
    Readable.toWeb(stdout) as ReadableStream<Uint8Array>,
  );

  return {
    child,
    stdin,
    stdout,
    stream,
    close: () => stopChild(child),
    [Symbol.asyncDispose]: () => stopChild(child),
  } satisfies AcpAgentProcess;
}
