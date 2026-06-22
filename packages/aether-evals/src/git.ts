import { env as processEnv } from "node:process";

import { runCommand } from "@aether-agent/sdk";

export class GitRepo {
  private constructor(readonly path: string) {}

  static fromPath(path: string): GitRepo {
    return new GitRepo(path);
  }

  static async clone(
    url: string,
    dest: string,
    signal?: AbortSignal,
  ): Promise<GitRepo> {
    await git(
      ["clone", "--no-checkout", "--filter=blob:none", url, dest],
      signal,
    );
    return new GitRepo(dest);
  }

  async checkout(reference: string, signal?: AbortSignal): Promise<void> {
    await git(["-C", this.path, "checkout", reference], signal);
  }

  async diff(
    fromCommit: string,
    toCommit?: string,
    signal?: AbortSignal,
  ): Promise<string> {
    const range = toCommit ? `${fromCommit}..${toCommit}` : fromCommit;
    const result = await git(["-C", this.path, "diff", range], signal);
    return result.stdout;
  }

  async diffUnstaged(signal?: AbortSignal): Promise<string> {
    return this.diff("HEAD", undefined, signal);
  }
}

async function git(args: string[], signal?: AbortSignal) {
  return await runCommand("git", args, {
    cwd: process.cwd(),
    env: processEnv,
    abortSignal: signal,
    spawnFailedMessage:
      "Failed to run git for the eval workspace (is git installed and on PATH?)",
    exitedErrorCode: "execution_failed",
    exitedMessage: ({ exitCode, stderr }) =>
      `git ${args.join(" ")} exited with code ${exitCode}.\n${stderr}`,
  });
}
