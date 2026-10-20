import { resolve } from "node:path";

import type { GitSource, GitSpec } from "@aether-agent/sdk";
import { toGitDiff, type GitDiff } from "./diff.js";
import { AetherEvalError } from "./errors.js";
import { createTempDir, pathExists, removeDir } from "./fs.js";
import { GitRepo } from "./git.js";
import { Workspace } from "./workspace.js";

const EVAL_START_REF = "eval-start";
const EVAL_GOLD_REF = "eval-gold";
const DEFAULT_BRANCH = "main";

export class GitFixture implements AsyncDisposable {
  readonly referenceDiff: GitDiff;
  readonly #repo: GitRepo;
  readonly #spec: GitSpec;
  #cleaned = false;

  private constructor(repo: GitRepo, spec: GitSpec, referenceDiff: GitDiff) {
    this.#repo = repo;
    this.#spec = spec;
    this.referenceDiff = referenceDiff;
  }

  static async fetch(spec: GitSpec, signal?: AbortSignal): Promise<GitFixture> {
    const { source, startCommit, goldCommit, diffExclude = [] } = spec;
    const from = await remote(source);

    const repoPath = await createTempDir("aether-eval-git-");
    try {
      const repo = await GitRepo.init(repoPath, signal);
      await repo.fetch(from, [startCommit, goldCommit], signal);
      await repo.updateRef(`refs/heads/${EVAL_START_REF}`, startCommit, signal);
      await repo.updateRef(`refs/heads/${EVAL_GOLD_REF}`, goldCommit, signal);
      const referenceDiff = toGitDiff(
        await repo.diff(startCommit, goldCommit, diffExclude, signal),
      );
      return new GitFixture(repo, spec, referenceDiff);
    } catch (err) {
      await removeDir(repoPath);
      throw err;
    }
  }

  workspace(signal?: AbortSignal): Promise<Workspace> {
    const { startCommit, branch = DEFAULT_BRANCH, subdir } = this.#spec;
    return Workspace.create(
      async (rootPath) => {
        const repo = await GitRepo.init(rootPath, signal);
        await repo.fetch(this.#repo.path, [startCommit], signal);
        await repo.checkoutBranch(branch, startCommit, signal);
      },
      { subdir: subdir ?? undefined },
    );
  }

  async agentDiff(
    workspace: Workspace,
    signal?: AbortSignal,
  ): Promise<GitDiff> {
    const { startCommit, diffExclude = [] } = this.#spec;
    const repo = GitRepo.fromPath(workspace.rootPath);
    return toGitDiff(
      await repo.diffWorkingTree(startCommit, diffExclude, signal),
    );
  }

  async bundle(outPath: string, signal?: AbortSignal): Promise<void> {
    await this.#repo.bundle(
      [EVAL_START_REF, EVAL_GOLD_REF],
      resolve(outPath),
      signal,
    );
  }

  async cleanup(): Promise<void> {
    if (this.#cleaned) return;
    this.#cleaned = true;
    await removeDir(this.#repo.path);
  }

  [Symbol.asyncDispose](): Promise<void> {
    return this.cleanup();
  }
}

async function remote(source: GitSource): Promise<string> {
  if ("url" in source) return source.url;
  const bundle = resolve(source.bundle);
  if (!(await pathExists(bundle))) {
    throw new AetherEvalError(
      "configuration_error",
      `git bundle file does not exist: ${bundle}`,
    );
  }
  return bundle;
}
