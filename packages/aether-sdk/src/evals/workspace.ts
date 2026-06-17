import { rm } from "node:fs/promises";

export interface RetainedWorkspace {
  /** Retained repository/temp root. Cleanup removes this directory. */
  readonly rootPath: string;

  /** Effective cwd where eval assertions ran. */
  readonly path: string;
}

/**
 * Handle to a retained eval workspace on the host.
 *
 * Implements `AsyncDisposable` so an `await using` binding removes the directory when it goes out of
 * scope. Call {@link WorkspaceHandle.cleanup} to remove it eagerly.
 */
export interface WorkspaceHandle extends RetainedWorkspace, AsyncDisposable {
  /** Remove the workspace directory. Idempotent; deletes even when retention was requested. */
  cleanup(): Promise<void>;
}

/**
 * Build a {@link WorkspaceHandle} for a workspace the CLI retained on the host. When
 * `keepOnDispose` is set, disposal logs the path and leaves the directory in place for debugging;
 * explicit {@link WorkspaceHandle.cleanup} always removes it.
 */
export function createWorkspaceHandle(
  retainedWorkspace: RetainedWorkspace,
  keepOnDispose: boolean,
): WorkspaceHandle {
  let cleaned = false;

  const cleanup = async (): Promise<void> => {
    if (cleaned) return;
    cleaned = true;
    await rm(retainedWorkspace.rootPath, { recursive: true, force: true });
  };

  const dispose = async (): Promise<void> => {
    if (keepOnDispose) {
      console.error(
        `[aether eval] retaining workspace for inspection: ${retainedWorkspace.path}`,
      );
      return;
    }
    await cleanup();
  };

  return { ...retainedWorkspace, cleanup, [Symbol.asyncDispose]: dispose };
}
