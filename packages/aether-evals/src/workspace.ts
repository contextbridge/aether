import { cp, mkdir, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";

import { AetherEvalError } from "./errors.js";
import { createTempDir, pathExists, removeDir } from "./fs.js";

export interface RetainedWorkspaceInfo {
  rootPath: string;
  path: string;
}

export interface WorkspaceOptions {
  subdir?: string;
}

/** A workspace created on the host for an eval run. */
export class Workspace implements AsyncDisposable {
  /** Retained repository/temp root. Cleanup removes this directory; bind-mounted to `/workspace`. */
  readonly rootPath: string;

  /** Effective cwd where eval assertions run (`rootPath` or `rootPath/<subdir>`). */
  readonly path: string;

  /** Subdirectory (relative to `rootPath`) used as the working dir, if any. */
  readonly relativeCwd?: string;

  #cleaned = false;

  private constructor(options: {
    rootPath: string;
    path: string;
    relativeCwd?: string;
  }) {
    this.rootPath = options.rootPath;
    this.path = options.path;
    this.relativeCwd = options.relativeCwd;
  }

  static async create(
    populate: (rootPath: string) => Promise<void>,
    { subdir }: WorkspaceOptions = {},
  ): Promise<Workspace> {
    const rootPath = await createTempDir("aether-eval-workspace-");
    try {
      await populate(rootPath);
      return new Workspace({
        rootPath,
        path: await resolveSubdir(rootPath, subdir),
        relativeCwd: subdir,
      });
    } catch (err) {
      await removeDir(rootPath);
      throw err;
    }
  }

  static empty(): Promise<Workspace> {
    return Workspace.create(async () => {});
  }

  static fromFiles(files: Record<string, string>): Promise<Workspace> {
    return Workspace.create(async (rootPath) => {
      for (const [relativePath, content] of Object.entries(files)) {
        const target = resolvePath(rootPath, relativePath);
        await mkdir(dirname(target), { recursive: true });
        await writeFile(target, content);
      }
    });
  }

  static fromDir(srcDir: string, baseDir = process.cwd()): Promise<Workspace> {
    return Workspace.create(async (rootPath) => {
      await cp(resolve(baseDir, srcDir), rootPath, { recursive: true });
    });
  }

  join(relativePath: string): string {
    return join(this.path, relativePath);
  }

  persist(): RetainedWorkspaceInfo {
    this.#cleaned = true;
    return { rootPath: this.rootPath, path: this.path };
  }

  /** Remove the workspace directory. Idempotent. */
  async cleanup(): Promise<void> {
    if (this.#cleaned) return;
    this.#cleaned = true;
    await removeDir(this.rootPath);
  }

  [Symbol.asyncDispose](): Promise<void> {
    return this.cleanup();
  }
}

function resolvePath(root: string, relativePath: string): string {
  const target = resolve(root, relativePath);
  const rel = relative(root, target);
  if (rel === "" || rel.startsWith("..")) {
    throw new AetherEvalError(
      "configuration_error",
      `workspace file path escapes the workspace root: ${relativePath}`,
    );
  }
  return target;
}

async function resolveSubdir(
  rootPath: string,
  subdir?: string,
): Promise<string> {
  if (!subdir) return rootPath;
  const path = join(rootPath, subdir);
  if (!(await pathExists(path))) {
    throw new AetherEvalError(
      "configuration_error",
      `workspace subdirectory does not exist: ${path}`,
    );
  }
  return path;
}
