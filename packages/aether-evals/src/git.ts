import { env as processEnv } from "node:process";

import { runCommand } from "@aether-agent/sdk";

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

  /**
   * Clone `source` (a URL or a local bundle/path) into `dest` with `--no-checkout`.
   *
   * When `blobless` (the default), uses a partial blobless clone (`--filter=blob:none`)
   * that defers blob downloads -- cheaper for large repos. Pass false to download the full
   * object set so the clone is self-contained (e.g. when `source` is a bundle, which is not
   * a promisor remote).
   */
  static async clone(
    source: string,
    dest: string,
    signal?: AbortSignal,
    blobless = true,
  ): Promise<GitRepo> {
    const filter = blobless ? ["--filter=blob:none"] : [];
    await git(["clone", "--no-checkout", ...filter, source, dest], signal);
    return new GitRepo(dest);
  }

  async checkout(reference: string, signal?: AbortSignal): Promise<void> {
    await git(["-C", this.path, "checkout", reference], signal);
  }

  /** Fetch `revs` from `remote` (a named remote or a URL) into this repository. */
  async fetch(
    remote: string,
    revs: string[],
    signal?: AbortSignal,
  ): Promise<void> {
    await git(["-C", this.path, "fetch", remote, ...revs], signal);
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
    toCommit?: string,
    signal?: AbortSignal,
  ): Promise<string> {
    const range = toCommit ? `${fromCommit}..${toCommit}` : fromCommit;
    const result = await git(["-C", this.path, "diff", range], signal);
    return result.stdout;
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
