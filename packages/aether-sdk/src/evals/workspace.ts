import { access, cp, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";

import { AetherSdkError } from "../errors.js";
import { diffStatsFromDiff, type GitDiff } from "./diff.js";
import { GitRepo } from "./git.js";

export interface GitRepoSpec {
  url: string;
  startCommit: string;
  goldCommit: string;
  subdir?: string;
}

export interface RetainedWorkspaceInfo {
  rootPath: string;
  path: string;
}

/** Where an eval workspace came from. Git workspaces carry the commits for diffing. */
export type WorkspaceSource =
  | "local"
  | { git: { url: string; startCommit: string; goldCommit: string } };

/** A workspace created on the host for an eval run. */
export class Workspace implements AsyncDisposable {
  /** Retained repository/temp root. Cleanup removes this directory; bind-mounted to `/workspace`. */
  readonly rootPath: string;

  /** Effective cwd where eval assertions run (`rootPath` or `rootPath/<subdir>`). */
  readonly path: string;

  /** Subdirectory (relative to `rootPath`) used as the working dir, if any. */
  readonly relativeCwd?: string;

  readonly source: WorkspaceSource;

  #cleaned = false;

  private constructor(options: {
    rootPath: string;
    path: string;
    relativeCwd?: string;
    source: WorkspaceSource;
  }) {
    this.rootPath = options.rootPath;
    this.path = options.path;
    this.relativeCwd = options.relativeCwd;
    this.source = options.source;
  }

  static async empty(): Promise<Workspace> {
    const rootPath = await createWorkspaceDir();
    return new Workspace({ rootPath, path: rootPath, source: "local" });
  }

  static async fromFiles(files: Record<string, string>): Promise<Workspace> {
    const workspace = await Workspace.empty();
    try {
      for (const [relativePath, content] of Object.entries(files)) {
        const target = resolvePath(workspace.rootPath, relativePath);
        await mkdir(dirname(target), { recursive: true });
        await writeFile(target, content);
      }
      return workspace;
    } catch (err) {
      await workspace.cleanup();
      throw err;
    }
  }

  static async fromDir(
    srcDir: string,
    baseDir = process.cwd(),
  ): Promise<Workspace> {
    const workspace = await Workspace.empty();
    try {
      await cp(resolve(baseDir, srcDir), workspace.rootPath, {
        recursive: true,
      });
      return workspace;
    } catch (err) {
      await workspace.cleanup();
      throw err;
    }
  }

  static async fromGitRepo(
    spec: GitRepoSpec,
    signal?: AbortSignal,
  ): Promise<Workspace> {
    const rootPath = await createWorkspaceDir();
    try {
      const { url, startCommit, goldCommit, subdir } = spec;
      const repo = await GitRepo.clone(url, rootPath, signal);
      await repo.checkout(startCommit, signal);
      const path = subdir ? join(rootPath, subdir) : rootPath;
      if (subdir && !(await pathExists(path))) {
        throw new AetherSdkError(
          "invalid_options",
          `git workspace subdirectory does not exist: ${path}`,
        );
      }
      return new Workspace({
        rootPath,
        path,
        relativeCwd: subdir ?? undefined,
        source: { git: { url, startCommit, goldCommit } },
      });
    } catch (err) {
      await rm(rootPath, { recursive: true, force: true });
      throw err;
    }
  }

  join(relativePath: string): string {
    return join(this.path, relativePath);
  }

  persist(): RetainedWorkspaceInfo {
    this.#cleaned = true;
    return { rootPath: this.rootPath, path: this.path };
  }

  async captureGitDiffs(): Promise<{
    agentDiff?: GitDiff;
    referenceDiff?: GitDiff;
  }> {
    if (this.source === "local") return {};

    const repo = GitRepo.fromPath(this.path);
    const { startCommit, goldCommit } = this.source.git;
    const [agentDiff, referenceDiff] = await Promise.all([
      captureDiff(() => repo.diffUnstaged()),
      captureDiff(() => repo.diff(startCommit, goldCommit)),
    ]);
    return { agentDiff, referenceDiff };
  }

  /** Remove the workspace directory. Idempotent. */
  async cleanup(): Promise<void> {
    if (this.#cleaned) return;
    this.#cleaned = true;
    await rm(this.rootPath, { recursive: true, force: true });
  }

  [Symbol.asyncDispose](): Promise<void> {
    return this.cleanup();
  }
}

async function captureDiff(
  getDiff: () => Promise<string>,
): Promise<GitDiff | undefined> {
  try {
    const diff = await getDiff();
    return { diff, stats: diffStatsFromDiff(diff) };
  } catch {
    return undefined;
  }
}

async function createWorkspaceDir(): Promise<string> {
  return mkdtemp(join(tmpdir(), "aether-eval-workspace-"));
}

function resolvePath(root: string, relativePath: string): string {
  const target = resolve(root, relativePath);
  const rel = relative(root, target);
  if (rel === "" || rel.startsWith("..")) {
    throw new AetherSdkError(
      "invalid_options",
      `workspace file path escapes the workspace root: ${relativePath}`,
    );
  }
  return target;
}

async function pathExists(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}
