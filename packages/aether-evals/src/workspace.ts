import { access, cp, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";

import { AetherSdkError } from "@aether-agent/sdk";
import { diffStatsFromDiff, type GitDiff } from "./diff.js";
import { GitRepo } from "./git.js";

const EVAL_START_REF = "eval-start";
const EVAL_GOLD_REF = "eval-gold";
const DEFAULT_BRANCH = "main";

export interface GitCheckoutSpec {
  startCommit: string;
  goldCommit: string;
  subdir?: string;
  branch?: string;
}

export interface GitRepoSpec extends GitCheckoutSpec {
  url: string;
}

export interface GitBundleSpec extends GitCheckoutSpec {
  bundlePath: string;
}

export interface RetainedWorkspaceInfo {
  rootPath: string;
  path: string;
}

/** A workspace created on the host for an eval run. */
export class Workspace implements AsyncDisposable {
  /** Retained repository/temp root. Cleanup removes this directory; bind-mounted to `/workspace`. */
  readonly rootPath: string;

  /** Effective cwd where eval assertions run (`rootPath` or `rootPath/<subdir>`). */
  readonly path: string;

  /** Subdirectory (relative to `rootPath`) used as the working dir, if any. */
  readonly relativeCwd?: string;

  readonly #git: GitBaseline | undefined;
  #cleaned = false;

  private constructor(options: {
    rootPath: string;
    path: string;
    relativeCwd?: string;
    git?: GitBaseline;
  }) {
    this.rootPath = options.rootPath;
    this.path = options.path;
    this.relativeCwd = options.relativeCwd;
    this.#git = options.git;
  }

  static async empty(): Promise<Workspace> {
    const rootPath = await createWorkspaceDir();
    return new Workspace({ rootPath, path: rootPath });
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
    return Workspace.fromGit(spec.url, spec, signal);
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
    if (!(await pathExists(spec.bundlePath))) {
      throw new AetherSdkError(
        "invalid_options",
        `git bundle file does not exist: ${spec.bundlePath}`,
      );
    }
    return Workspace.fromGit(spec.bundlePath, spec, signal);
  }

  private static async fromGit(
    remote: string,
    spec: GitCheckoutSpec,
    signal?: AbortSignal,
  ): Promise<Workspace> {
    const { startCommit, goldCommit, subdir, branch = DEFAULT_BRANCH } = spec;
    return withTempDir(async (scratchPath) => {
      const scratch = await GitRepo.init(scratchPath, signal);
      await scratch.fetch(remote, [startCommit, goldCommit], signal);
      const referenceDiff = toGitDiff(
        await scratch.diff(startCommit, goldCommit, signal),
      );

      const rootPath = await createWorkspaceDir();
      try {
        const repo = await GitRepo.init(rootPath, signal);
        await repo.fetch(scratchPath, [startCommit], signal);
        await repo.checkoutBranch(branch, startCommit, signal);
        return new Workspace({
          rootPath,
          path: await resolveSubdir(rootPath, subdir),
          relativeCwd: subdir,
          git: { startCommit, referenceDiff },
        });
      } catch (err) {
        await rm(rootPath, { recursive: true, force: true });
        throw err;
      }
    });
  }

  join(relativePath: string): string {
    return join(this.path, relativePath);
  }

  persist(): RetainedWorkspaceInfo {
    this.#cleaned = true;
    return { rootPath: this.rootPath, path: this.path };
  }

  get referenceDiff(): GitDiff | undefined {
    return this.#git?.referenceDiff;
  }

  async captureAgentDiff(signal?: AbortSignal): Promise<GitDiff | undefined> {
    if (!this.#git) return undefined;
    const diff = await GitRepo.fromPath(this.rootPath).diff(
      this.#git.startCommit,
      undefined,
      signal,
    );
    return toGitDiff(diff);
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
  await withTempDir(async (tempDir) => {
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
  });
}

interface GitBaseline {
  startCommit: string;
  referenceDiff: GitDiff;
}

function toGitDiff(diff: string): GitDiff {
  return { diff, stats: diffStatsFromDiff(diff) };
}

async function createWorkspaceDir(): Promise<string> {
  return mkdtemp(join(tmpdir(), "aether-eval-workspace-"));
}

async function withTempDir<T>(fn: (path: string) => Promise<T>): Promise<T> {
  const path = await createWorkspaceDir();
  try {
    return await fn(path);
  } finally {
    await rm(path, { recursive: true, force: true });
  }
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
