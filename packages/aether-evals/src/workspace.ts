import { access, cp, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";

import { AetherSdkError } from "@aether-agent/sdk";
import { diffStatsFromDiff, type GitDiff } from "./diff.js";
import { GitRepo } from "./git.js";

const EVAL_START_REF = "eval-start";
const EVAL_GOLD_REF = "eval-gold";

export interface GitRepoSpec {
  url: string;
  startCommit: string;
  goldCommit: string;
  subdir?: string;
}

export interface GitBundleSpec {
  bundlePath: string;
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
  | { git: { url: string; startCommit: string; goldCommit: string } }
  | { bundle: { startCommit: string; goldCommit: string } };

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
    const { url, startCommit, goldCommit, subdir } = spec;
    return Workspace.fromClone(
      (rootPath) => GitRepo.clone(url, rootPath, signal),
      startCommit,
      subdir,
      { git: { url, startCommit, goldCommit } },
      signal,
    );
  }

  /**
   * Instantiate a workspace from a local git bundle file (see {@link createGitBundle}).
   *
   * The bundle must contain both `startCommit` and `goldCommit` so that checkout and
   * reference diffing work without the originating remote.
   */
  static async fromGitBundle(
    spec: GitBundleSpec,
    signal?: AbortSignal,
  ): Promise<Workspace> {
    const { bundlePath, startCommit, goldCommit, subdir } = spec;
    if (!(await pathExists(bundlePath))) {
      throw new AetherSdkError(
        "invalid_options",
        `git bundle file does not exist: ${bundlePath}`,
      );
    }
    return Workspace.fromClone(
      (rootPath) => GitRepo.clone(bundlePath, rootPath, signal, false),
      startCommit,
      subdir,
      { bundle: { startCommit, goldCommit } },
      signal,
    );
  }

  private static async fromClone(
    clone: (rootPath: string) => Promise<GitRepo>,
    startCommit: string,
    subdir: string | undefined,
    source: WorkspaceSource,
    signal?: AbortSignal,
  ): Promise<Workspace> {
    const rootPath = await createWorkspaceDir();
    try {
      const repo = await clone(rootPath);
      await repo.checkout(startCommit, signal);
      const path = await resolveSubdir(rootPath, subdir);
      return new Workspace({
        rootPath,
        path,
        relativeCwd: subdir ?? undefined,
        source,
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
    const commits = diffCommits(this.source);
    if (!commits) return {};

    const repo = GitRepo.fromPath(this.path);
    const { startCommit, goldCommit } = commits;
    const [agentDiff, referenceDiff] = await Promise.all([
      captureDiff(() => repo.diff(startCommit)),
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

/**
 * Create a self-contained git bundle at `outPath` containing `spec`'s start and gold
 * commits.
 *
 * Fetches only the start and gold commits (and their reachable objects) into a fresh repo,
 * then writes a bundle that {@link Workspace.fromGitBundle} can later instantiate offline.
 */
export async function createGitBundle(
  spec: GitRepoSpec,
  outPath: string,
  signal?: AbortSignal,
): Promise<void> {
  const tempDir = await createWorkspaceDir();
  try {
    const repo = await GitRepo.init(tempDir, signal);
    await repo.fetch(spec.url, [spec.startCommit, spec.goldCommit], signal);
    await repo.updateRef(
      `refs/heads/${EVAL_START_REF}`,
      spec.startCommit,
      signal,
    );
    await repo.updateRef(
      `refs/heads/${EVAL_GOLD_REF}`,
      spec.goldCommit,
      signal,
    );
    await repo.bundle([EVAL_START_REF, EVAL_GOLD_REF], outPath, signal);
  } finally {
    await rm(tempDir, { recursive: true, force: true });
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

async function resolveSubdir(
  rootPath: string,
  subdir?: string,
): Promise<string> {
  if (!subdir) return rootPath;
  const path = join(rootPath, subdir);
  if (!(await pathExists(path))) {
    throw new AetherSdkError(
      "invalid_options",
      `git workspace subdirectory does not exist: ${path}`,
    );
  }
  return path;
}

function diffCommits(
  source: WorkspaceSource,
): { startCommit: string; goldCommit: string } | undefined {
  if (source === "local") return undefined;
  return "git" in source ? source.git : source.bundle;
}
