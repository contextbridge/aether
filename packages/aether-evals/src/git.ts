import { copyFile } from "node:fs/promises";
import { join } from "node:path";
import { env as processEnv } from "node:process";

import { runCommand } from "@aether-agent/sdk";
import { withTempDir } from "./fs.js";

export class GitRepo {
  private constructor(readonly path: string) {}

  static fromPath(path: string): GitRepo {
    return new GitRepo(path);
  }

  /** Initialize an empty repository at `dest`. */
  static async init(dest: string, signal?: AbortSignal): Promise<GitRepo> {
    await git(["init", dest], signal);
    return new GitRepo(dest);
  }

  async checkoutBranch(
    branch: string,
    commit: string,
    signal?: AbortSignal,
  ): Promise<void> {
    await git(["-C", this.path, "checkout", "-B", branch, commit], signal);
  }

  /** Fetch `revs` from `remote` (a named remote or a URL) into this repository. */
  async fetch(
    remote: string,
    revs: string[],
    signal?: AbortSignal,
  ): Promise<void> {
    await git(["-C", this.path, "fetch", "--no-tags", remote, ...revs], signal);
  }

  async updateRef(
    name: string,
    commit: string,
    signal?: AbortSignal,
  ): Promise<void> {
    await git(["-C", this.path, "update-ref", name, commit], signal);
  }

  /**
   * Create a self-contained git bundle at `out` containing the given refs and their
   * reachable objects.
   */
  async bundle(
    revs: string[],
    out: string,
    signal?: AbortSignal,
  ): Promise<void> {
    await git(["-C", this.path, "bundle", "create", out, ...revs], signal);
  }

  async diff(
    fromCommit: string,
    toCommit: string,
    exclude: string[],
    signal?: AbortSignal,
  ): Promise<string> {
    return git(
      ["-C", this.path, "diff", fromCommit, toCommit, ...pathspecs(exclude)],
      signal,
    );
  }

  diffWorkingTree(
    fromCommit: string,
    exclude: string[],
    signal?: AbortSignal,
  ): Promise<string> {
    return withTempDir("aether-eval-index-", async (scratch) => {
      const env = { GIT_INDEX_FILE: join(scratch, "index") };
      try {
        await copyFile(join(this.path, ".git", "index"), env.GIT_INDEX_FILE);
      } catch (err) {
        if ((err as NodeJS.ErrnoException).code !== "ENOENT") throw err;
        await git(["-C", this.path, "read-tree", "HEAD"], signal, env);
      }
      await git(["-C", this.path, "add", "--all"], signal, env);
      return git(
        [
          "-C",
          this.path,
          "diff",
          "--cached",
          fromCommit,
          ...pathspecs(exclude),
        ],
        signal,
        env,
      );
    });
  }
}

function pathspecs(exclude: string[]): string[] {
  return exclude.length > 0
    ? ["--", ...exclude.map((pathspec) => `:(exclude)${pathspec}`)]
    : [];
}

function git(
  args: string[],
  signal?: AbortSignal,
  env: Record<string, string> = {},
): Promise<string> {
  return runCommand("git", args, {
    cwd: process.cwd(),
    env: { ...processEnv, ...env },
    abortSignal: signal,
    spawnFailedMessage:
      "Failed to run git for the eval workspace (is git installed and on PATH?)",
    exitedErrorCode: "execution_failed",
    exitedMessage: ({ exitCode, stderr }) =>
      `git ${args.join(" ")} exited with code ${exitCode}.\n${stderr}`,
  });
}
