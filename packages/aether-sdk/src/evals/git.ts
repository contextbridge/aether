import { env as processEnv } from "node:process";

import { runCommand } from "../childProcess.js";

export async function gitClone(
  url: string,
  ref: string,
  dest: string,
  signal?: AbortSignal,
): Promise<void> {
  await git(
    ["clone", "--no-checkout", "--filter=blob:none", url, dest],
    signal,
  );
  await git(["-C", dest, "checkout", ref], signal);
}

async function git(args: string[], signal?: AbortSignal): Promise<void> {
  await runCommand("git", args, {
    cwd: process.cwd(),
    env: processEnv,
    abortSignal: signal,
    spawnFailedMessage:
      "Failed to run git for the eval workspace (is git installed and on PATH?)",
    exitedErrorCode: "eval_command_failed",
    exitedMessage: ({ exitCode, stderr }) =>
      `git ${args.join(" ")} exited with code ${exitCode}.\n${stderr}`,
  });
}
